use mumei_core::emitter::Emitter;
use mumei_core::hir::{lower_atom_to_hir_with_env, HirAtom};
use mumei_core::parser::ast::Atom;
use mumei_core::parser::{parse_module_checked, Item};
use mumei_core::trust_boundary::{
    classify_trust_boundaries, extern_fn_as_trusted_atom, TrustBoundaryKind,
};
use mumei_core::verification::ModuleEnv;
use mumei_emit_monitor::RuntimeMonitorEmitter;
use std::path::Path;

struct MonitorGolden {
    name: &'static str,
    source: &'static str,
    qualified_name: Option<&'static str>,
    extern_fn: bool,
    expected_boundaries: &'static [TrustBoundaryKind],
    expected_artifact_count: usize,
    golden: Option<&'static [u8]>,
}

fn parsed_case(case: &MonitorGolden) -> (Atom, ModuleEnv) {
    let items = parse_module_checked(case.source).unwrap_or_else(|errors| {
        panic!("{} should parse: {}", case.name, errors.join("\n"));
    });
    let mut module_env = ModuleEnv::new();
    let mut atom = None;
    for item in items {
        match item {
            Item::Atom(parsed) => atom = Some(parsed),
            Item::ImplBlock(block) => atom = block.methods.into_iter().next(),
            Item::ExternBlock(block) => {
                if case.extern_fn {
                    atom = Some(extern_fn_as_trusted_atom(
                        block.functions.first().expect("extern function"),
                    ));
                }
                module_env.register_extern_block(&block);
            }
            _ => {}
        }
    }
    let mut atom = atom.expect("source should contain an atom");
    if let Some(name) = case.qualified_name {
        atom.name = name.to_string();
    }
    (atom, module_env)
}

fn parsed_unrelated_atom() -> Atom {
    let items = parse_module_checked(
        r#"
atom unrelated() -> bool
requires: false;
ensures: false;
body: { false }
"#,
    )
    .expect("unrelated atom should parse");
    items
        .into_iter()
        .find_map(|item| match item {
            Item::Atom(atom) => Some(atom),
            _ => None,
        })
        .expect("unrelated atom")
}

fn emit_monitor(
    hir: &HirAtom,
    case: &MonitorGolden,
    module_env: &ModuleEnv,
    extern_blocks: &[mumei_core::parser::ExternBlock],
) -> Vec<Vec<u8>> {
    RuntimeMonitorEmitter
        .emit(hir, Path::new(case.name), module_env, extern_blocks)
        .unwrap_or_else(|error| panic!("{} should emit: {error}", case.name))
        .into_iter()
        .map(|artifact| artifact.data)
        .collect()
}

#[test]
fn parser_backed_runtime_monitor_outputs_pin_develop_and_ignore_the_ast() {
    let cases = [
        MonitorGolden {
            name: "trusted_plain",
            source: r#"
trusted atom trusted_plain(x: i64) -> i64
requires: x >= 0;
ensures: result >= x;
body: { x }
"#,
            qualified_name: None,
            extern_fn: false,
            expected_boundaries: &[TrustBoundaryKind::TrustedAtom],
            expected_artifact_count: 1,
            golden: Some(include_bytes!(
                "fixtures/runtime_monitor/goldens/trusted_plain.rs"
            )),
        },
        MonitorGolden {
            name: "assumed_requires",
            source: r#"
atom assumed_requires(x: i64) -> i64
requires assume: x > 0;
requires: x < 100;
ensures: result >= 0;
body: { x }
"#,
            qualified_name: None,
            extern_fn: false,
            expected_boundaries: &[TrustBoundaryKind::AssumedClause],
            expected_artifact_count: 1,
            golden: Some(include_bytes!(
                "fixtures/runtime_monitor/goldens/assumed_requires.rs"
            )),
        },
        MonitorGolden {
            name: "assumed_ensures",
            source: r#"
atom assumed_ensures(x: i64) -> i64
requires: x > 0;
ensures assume: result > 0;
ensures: result < 100;
body: { x }
"#,
            qualified_name: None,
            extern_fn: false,
            expected_boundaries: &[TrustBoundaryKind::AssumedClause],
            expected_artifact_count: 1,
            golden: Some(include_bytes!(
                "fixtures/runtime_monitor/goldens/assumed_ensures.rs"
            )),
        },
        MonitorGolden {
            name: "check_only",
            source: r#"
atom check_only(x: i64) -> i64
requires check: x > 0;
ensures check: result >= 0;
body: { x }
"#,
            qualified_name: None,
            extern_fn: false,
            expected_boundaries: &[],
            expected_artifact_count: 0,
            golden: None,
        },
        MonitorGolden {
            name: "trusted_assumed",
            source: r#"
trusted atom trusted_assumed(x: i64) -> i64
requires assume: x > 0;
requires: x < 100;
ensures: result >= 0;
body: { x }
"#,
            qualified_name: None,
            extern_fn: false,
            expected_boundaries: &[
                TrustBoundaryKind::TrustedAtom,
                TrustBoundaryKind::AssumedClause,
            ],
            expected_artifact_count: 1,
            golden: Some(include_bytes!(
                "fixtures/runtime_monitor/goldens/trusted_assumed.rs"
            )),
        },
        MonitorGolden {
            name: "cover",
            source: r#"
trusted atom covered(x: i64) -> i64
requires: x >= 0;
ensures: result >= 0;
cover "positive": x > 0;
body: { x }
"#,
            qualified_name: None,
            extern_fn: false,
            expected_boundaries: &[TrustBoundaryKind::TrustedAtom],
            expected_artifact_count: 1,
            golden: Some(include_bytes!("fixtures/runtime_monitor/goldens/cover.rs")),
        },
        MonitorGolden {
            name: "effect_pre",
            source: r#"
atom effectful(x: i64) -> i64
effect_pre: { Zeta: Ready, Alpha: Idle };
requires: x >= 0;
ensures: result == x;
body: { x }
"#,
            qualified_name: None,
            extern_fn: false,
            expected_boundaries: &[TrustBoundaryKind::EffectStateAssumption],
            expected_artifact_count: 1,
            golden: Some(include_bytes!(
                "fixtures/runtime_monitor/goldens/effect_pre.rs"
            )),
        },
        MonitorGolden {
            name: "extern_boundary",
            source: r#"
extern "C" {
    fn read_channel(channel: i64) -> i64
        requires: channel >= 0;
        ensures: result >= 0;
}
"#,
            qualified_name: None,
            extern_fn: true,
            expected_boundaries: &[
                TrustBoundaryKind::TrustedAtom,
                TrustBoundaryKind::ExternBoundary,
            ],
            expected_artifact_count: 1,
            golden: Some(include_bytes!(
                "fixtures/runtime_monitor/goldens/extern_boundary.rs"
            )),
        },
        MonitorGolden {
            name: "qualified_params",
            source: r#"
impl Vec2 {
trusted atom dot(
    consume a: i64,
    ref b: i64,
    ref mut c: i64,
    plain
) -> i64
requires: a >= 0;
ensures: result == a;
body: { a }
}
"#,
            qualified_name: Some("Vec2::dot"),
            extern_fn: false,
            expected_boundaries: &[TrustBoundaryKind::TrustedAtom],
            expected_artifact_count: 1,
            golden: Some(include_bytes!(
                "fixtures/runtime_monitor/goldens/qualified_params.rs"
            )),
        },
        MonitorGolden {
            name: "unsupported_contracts",
            source: r##"
trusted atom unsupported(x: i64, text: i64) -> i64
requires: x > 0 || forall(i, 0, x, i >= 0) && text == "ok";
ensures: result >= 0 -> result < 10;
body: { x }
"##,
            qualified_name: None,
            extern_fn: false,
            expected_boundaries: &[TrustBoundaryKind::TrustedAtom],
            expected_artifact_count: 1,
            golden: Some(include_bytes!(
                "fixtures/runtime_monitor/goldens/unsupported_contracts.rs"
            )),
        },
    ];

    for case in &cases {
        let (atom, module_env) = parsed_case(case);
        let hir = lower_atom_to_hir_with_env(&atom, Some(&module_env));
        assert_eq!(
            hir.meta.trust_boundaries,
            classify_trust_boundaries(&atom, &module_env.extern_blocks),
            "{} metadata boundaries",
            case.name
        );
        assert_eq!(
            hir.meta.trust_boundaries, case.expected_boundaries,
            "{} expected boundaries",
            case.name
        );
        let actual = emit_monitor(&hir, case, &module_env, &module_env.extern_blocks);
        assert_eq!(
            actual.len(),
            case.expected_artifact_count,
            "{} artifact count",
            case.name
        );
        match (actual.first(), case.golden) {
            (Some(bytes), Some(expected)) => assert_eq!(
                bytes.as_slice(),
                expected,
                "{} byte-exact develop golden",
                case.name
            ),
            (None, None) => {}
            _ => panic!("{} golden/artifact count mismatch", case.name),
        }

        let mut unrelated = hir.clone();
        unrelated.atom = parsed_unrelated_atom();
        let changed = emit_monitor(&unrelated, case, &module_env, &[]);
        assert_eq!(
            changed.len(),
            actual.len(),
            "{} AST independence artifact count",
            case.name
        );
        assert_eq!(
            changed, actual,
            "{} AST/extern-block independence",
            case.name
        );
    }
}
