use mumei_core::hir::lower_atom_to_hir;
use mumei_core::parser::{parse_module_checked, Item};
use mumei_core::verification::ModuleEnv;
use mumei_emit_llvm::translation_validation::{self as tv, Options, Outcome};

const CLAMP: &str = include_str!("fixtures/semantic_contract_clamp.mm");

fn pin(ir: &str) -> String {
    format!(
        "target triple = \"{}\"\ntarget datalayout = \"{}\"\n{ir}",
        tv::TARGET,
        tv::DATA_LAYOUT
    )
}

fn source(params: &str, body: &str) -> String {
    format!("atom f({params}) -> i64 requires: true; ensures: true; body: {{ {body} }};")
}

fn emitted(source: &str) -> String {
    let Item::Atom(atom) = parse_module_checked(source).unwrap().remove(0) else {
        panic!("atom")
    };
    let hir = lower_atom_to_hir(&atom);
    pin(
        &mumei_emit_llvm::codegen::compile_to_module(&hir.emit_view(), &ModuleEnv::default(), &[])
            .unwrap(),
    )
}

fn validate(source: &str, ir: &str) -> tv::ValidationReport {
    tv::validate_pure_i64(source, ir, Options::default()).unwrap()
}

fn function(body: &str) -> String {
    pin(&format!("define i64 @f(i64 %x) {{ {body} }}"))
}

#[test]
fn actual_emitter_clamp_is_equivalent_over_all_i64_inputs() {
    let ir = emitted(CLAMP);
    let report = validate(CLAMP, &ir);
    assert_eq!(report.outcome, Outcome::EquivalentWithinProfile);
    assert_eq!(report.profile, tv::PROFILE);
    assert_eq!(report.target, tv::TARGET);
    assert_eq!(report.data_layout, tv::DATA_LAYOUT);
    assert_eq!(
        report.checked_stage,
        "supplied_llvm_ir_snapshot_no_pipeline_attestation"
    );
    assert_eq!(
        report.input_domain,
        "all_signed_i64_inputs_not_restricted_by_requires"
    );
    assert_eq!(
        report.source_sha256,
        mumei_core::native_bundle::sha256(CLAMP.as_bytes())
    );
    assert_eq!(
        report.llvm_sha256,
        mumei_core::native_bundle::sha256(ir.as_bytes())
    );
}

#[test]
fn real_emitter_comparison_boolean_and_nested_branch_matrix() {
    for op in ["<", "<=", ">", ">=", "==", "!="] {
        for params in ["x: i64", "x: i64, y: i64"] {
            let rhs = if params.contains('y') { "y" } else { "0" };
            for condition in [
                format!("x {op} {rhs}"),
                format!("(x {op} {rhs}) && (x >= 0)"),
                format!("(x {op} {rhs}) || (x >= 0)"),
                format!("(x {op} {rhs}) == (x >= 0)"),
                format!("if x > 0 {{ x {op} {rhs} }} else {{ false }}"),
            ] {
                let src = source(
                    params,
                    &format!("if {condition} {{ if x == 7 {{ 99 }} else {{ x }} }} else {{ 42 }}"),
                );
                assert_eq!(
                    validate(&src, &emitted(&src)).outcome,
                    Outcome::EquivalentWithinProfile,
                    "{condition}"
                );
            }
        }
    }
    for body in [
        "x",
        "0",
        "9223372036854775807",
        "if true { x } else { 7 }",
        "if false { 7 } else { x }",
    ] {
        let src = source("x: i64", body);
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile
        );
    }
    let src = source("", "42");
    assert_eq!(
        validate(&src, &emitted(&src)).outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn wrong_comparisons_unsigned_lowering_and_branch_phi_replacements_are_refuted() {
    let ir = emitted(CLAMP);
    for replacement in ["sgt", "sge", "eq", "ne", "ult", "ugt"] {
        let wrong = ir.replace("icmp slt", &format!("icmp {replacement}"));
        assert_ne!(wrong, ir);
        assert_eq!(
            validate(CLAMP, &wrong).outcome,
            Outcome::Different,
            "{replacement}"
        );
    }
    // At zero both clamp branches yield zero; textual difference is not inequivalence.
    assert_eq!(
        validate(CLAMP, &ir.replace("icmp slt", "icmp sle")).outcome,
        Outcome::EquivalentWithinProfile
    );
    assert_eq!(
        validate(
            CLAMP,
            &pin("define i64 @clamp_nonnegative(i64 %x) { ret i64 %x }")
        )
        .outcome,
        Outcome::Different
    );
    assert_eq!(
        validate(
            CLAMP,
            &pin("define i64 @clamp_nonnegative(i64 %x) { ret i64 0 }")
        )
        .outcome,
        Outcome::Different
    );
}

#[test]
fn equivalence_does_not_prove_contracts_or_accept_false_preconditions_vacuously() {
    let ir = emitted(CLAMP);
    let false_post = CLAMP.replace("(x < 0 => result == 0) && (x >= 0 => result == x)", "false");
    let false_pre = CLAMP.replace("requires: true", "requires: false");
    assert_eq!(
        validate(&false_post, &ir).outcome,
        Outcome::EquivalentWithinProfile
    );
    assert_eq!(
        validate(&false_pre, &ir.replace("icmp slt", "icmp sgt")).outcome,
        Outcome::Different
    );
    assert_ne!(
        validate(CLAMP, &ir).source_sha256,
        validate(&false_post, &ir).source_sha256
    );
}

#[test]
fn argument_order_is_positional_not_llvm_identifier_spelling() {
    let src = source("x: i64, y: i64", "x");
    let good = pin("define i64 @f(i64 %y, i64 %x) { ret i64 %y }");
    let wrong = pin("define i64 @f(i64 %y, i64 %x) { ret i64 %x }");
    assert_eq!(
        validate(&src, &good).outcome,
        Outcome::EquivalentWithinProfile
    );
    assert_eq!(validate(&src, &wrong).outcome, Outcome::Different);
}

#[test]
fn differences_on_rare_and_signed_boundary_inputs_are_not_finite_replay() {
    let src = source("x: i64", "x");
    for value in ["123456789", "-9223372036854775808", "9223372036854775807"] {
        let ir = function(&format!(
            "%c = icmp eq i64 %x, {value}\n%v = select i1 %c, i64 0, i64 %x\nret i64 %v"
        ));
        assert_eq!(validate(&src, &ir).outcome, Outcome::Different);
    }
}

#[test]
fn selection_cast_and_bitwise_ssa_encodings_are_checked_independently() {
    let src = source("x: i64", "if x < 0 { 0 } else { x }");
    for body in [
        "%c = icmp slt i64 %x, 0\n%v = select i1 %c, i64 0, i64 %x\nret i64 %v",
        "%c = icmp slt i64 %x, 0\n%z = zext i1 %c to i64\n%t = trunc i64 %z to i1\n%a = and i1 %t, true\n%o = or i1 %a, false\n%n = xor i1 %o, false\n%v = select i1 %n, i64 0, i64 %x\nret i64 %v",
    ] {
        assert_eq!(validate(&src, &function(body)).outcome, Outcome::EquivalentWithinProfile);
    }
    let wrong = function("%v = and i64 %x, 9223372036854775807\nret i64 %v");
    assert_eq!(validate(&src, &wrong).outcome, Outcome::Different);
}

#[test]
fn source_implication_is_checked_without_claiming_emitter_support() {
    let src = source("x: i64", "if (x < 0) => (x > 0) { 1 } else { x }");
    let ir = function("%a = icmp slt i64 %x, 0\n%b = icmp sgt i64 %x, 0\n%n = xor i1 %a, true\n%c = or i1 %n, %b\n%v = select i1 %c, i64 1, i64 %x\nret i64 %v");
    assert_eq!(
        validate(&src, &ir).outcome,
        Outcome::EquivalentWithinProfile
    );
    let wrong = ir.replace("xor i1 %a, true", "xor i1 %a, false");
    assert_eq!(validate(&src, &wrong).outcome, Outcome::Different);
    let Item::Atom(atom) = parse_module_checked(&src).unwrap().remove(0) else {
        panic!("atom")
    };
    let hir = lower_atom_to_hir(&atom);
    assert!(mumei_emit_llvm::codegen::compile_to_module(
        &hir.emit_view(),
        &ModuleEnv::default(),
        &[]
    )
    .is_err());
}

#[test]
fn multiple_returns_and_phi_predecessors_are_not_merged_by_value_name() {
    let src = source("x: i64", "if x < 0 { 0 } else { x }");
    let ir = function("entry: %c = icmp slt i64 %x, 0\nbr i1 %c, label %neg, label %pos\nneg: ret i64 0\npos: ret i64 %x");
    assert_eq!(
        validate(&src, &ir).outcome,
        Outcome::EquivalentWithinProfile
    );
    let ir = function("entry: %c = icmp slt i64 %x, 0\nbr i1 %c, label %neg, label %pos\nneg: br label %end\npos: br label %end\nend: %p = phi i64 [0, %neg], [%x, %pos]\nret i64 %p");
    assert_eq!(
        validate(&src, &ir).outcome,
        Outcome::EquivalentWithinProfile
    );
    let wrong = ir.replace("[0, %neg], [%x, %pos]", "[%x, %neg], [0, %pos]");
    assert_eq!(validate(&src, &wrong).outcome, Outcome::Different);
}

#[test]
fn unsupported_source_is_not_silently_weakened() {
    for body in [
        "x/2",
        "x%2",
        "x<<1",
        "1.0",
        "let y = x; y",
        "f(x)",
        "while x > 0 invariant true { x }; x",
    ] {
        assert!(tv::validate_pure_i64(
            &source("x: i64", body),
            &function("ret i64 %x"),
            Options::default()
        )
        .is_err());
    }
    for params in ["x: u64", "x: bool", "x: [i64]", "x: f64"] {
        assert!(tv::validate_pure_i64(
            &source(params, "0"),
            &function("ret i64 0"),
            Options::default()
        )
        .is_err());
    }
}

#[test]
fn wrapping_i64_arithmetic_is_equivalent_and_refuted() {
    for (params, body) in [
        ("x: i64", "x + 1"),
        ("x: i64", "x - 1"),
        ("x: i64", "x * 2"),
        ("x: i64, y: i64", "x + y"),
        ("x: i64, y: i64", "x - y"),
        ("x: i64, y: i64", "x * y"),
        ("x: i64, y: i64", "x * x - y"),
        ("x: i64, y: i64", "(x + y) * (x - y)"),
        ("x: i64", "if x + 1 > 0 { x * 2 } else { 0 - x }"),
        // Wrap: addition of a negative literal lowers to `sub`; -x to `0 - x`.
        ("x: i64", "x + 0 - 1"),
    ] {
        let src = source(params, body);
        assert_eq!(
            validate(&src, &emitted(&src)).outcome,
            Outcome::EquivalentWithinProfile,
            "{body}"
        );
    }
    // Nonlinear cases that outrun the solver budget fail closed as
    // inconclusive — never weakened into equivalence or difference.
    let guarded = source("x: i64", "if x + 1 > 0 { x * x } else { 0 - x }");
    assert_eq!(
        validate(&guarded, &emitted(&guarded)).outcome,
        Outcome::Inconclusive
    );
    // Deliberately wrong lowering is detected, including rare-input diffs.
    let src = source("x: i64, y: i64", "x + y");
    let ir = emitted(&src);
    for wrong in [
        ir.replace("add i64", "sub i64"),
        ir.replace("add i64 %0, %1", "add i64 %0, 1"),
        ir.replace("add i64 %0, %1", "add i64 %1, 0"),
    ] {
        assert_ne!(wrong, ir);
        assert_eq!(
            validate(&src, &wrong).outcome,
            Outcome::Different,
            "{wrong}"
        );
    }
    // x*2 and x+x are equal for every input — a textual difference is not a bug.
    let doubled = source("x: i64", "x + x");
    assert_eq!(
        validate(&doubled, &emitted(&source("x: i64", "x * 2"))).outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn unsupported_llvm_memory_calls_arithmetic_division_shift_and_freeze_are_rejected() {
    let src = source("x: i64", "x");
    for body in [
        // Flagged arithmetic produces poison on overflow — stays rejected.
        "%v = add nsw i64 %x, 0\nret i64 %v",
        "%v = add nuw i64 %x, 0\nret i64 %v",
        "%v = sub nsw i64 %x, 0\nret i64 %v",
        "%v = mul nuw i64 %x, 1\nret i64 %v",
        "%v = sdiv i64 %x, 1\nret i64 %v",
        "%v = urem i64 %x, 1\nret i64 %v",
        "%v = shl i64 %x, 0\nret i64 %v",
        "%v = ashr exact i64 %x, 0\nret i64 %v",
        "%v = freeze i64 %x\nret i64 %v",
        "%p = alloca i64\nstore i64 %x, ptr %p\n%v = load i64, ptr %p\nret i64 %v",
        "%v = call i64 @f(i64 %x)\nret i64 %v",
        "unreachable",
    ] {
        assert!(
            tv::validate_pure_i64(&src, &function(body), Options::default()).is_err(),
            "{body}"
        );
    }
}

#[test]
fn undef_poison_and_cycles_are_rejected_even_in_dead_blocks_or_branches() {
    let src = source("x: i64", "x");
    for body in [
        "ret i64 undef",
        "ret i64 poison",
        "%v = select i1 true, i64 %x, i64 poison\nret i64 %v",
        "entry: ret i64 %x\ndead: ret i64 undef",
        "entry: ret i64 %x\ndead: %p = icmp eq ptr null, null\nret i64 0",
        "entry: br label %loop\nloop: br label %loop",
        "entry: ret i64 %x\ndead: br label %dead",
    ] {
        assert!(tv::validate_pure_i64(&src, &function(body), Options::default()).is_err());
    }
}

#[test]
fn target_signature_module_and_metadata_mismatches_are_rejected() {
    let src = source("x: i64", "x");
    let ir = function("ret i64 %x");
    let variants = [
        ir.replace(tv::TARGET, "aarch64-unknown-linux-gnu"),
        ir.replace(tv::DATA_LAYOUT, "e-p:64:64"),
        "define i64 @f(i64 %x) { ret i64 %x }".to_owned(),
        ir.replace("@f", "@g"),
        ir.replace("i64 %x", "i32 %x"),
        ir.replace("i64 %x)", "i64 %x, ...)"),
        ir.replace("define i64", "define fastcc i64"),
        ir.replace("define i64", "define noundef i64"),
        ir.replace("i64 %x)", "i64 noundef %x)"),
        ir.replace(") {", ") noreturn {"),
        format!("{ir}\n@g = global i64 0"),
        format!("{ir}\n@a = alias i64 (i64), ptr @f"),
        format!("{ir}\ndeclare i64 @g(i64)"),
        format!("{ir}\ndefine i64 @g(i64 %x) {{ret i64 %x}}"),
        format!("{ir}\nmodule asm \"nop\""),
        ir.replace("ret i64 %x", "ret i64 %x, !annotation !0") + "\n!0 = !{!\"excluded\"}",
    ];
    for input in variants {
        assert!(tv::validate_pure_i64(&src, &input, Options::default()).is_err());
    }
}

#[test]
fn malformed_ir_and_wrong_ssa_dominance_fail_without_echoing_input() {
    let src = source("x: i64", "x");
    for input in ["not IR", "define i64 @f(i64 %x) {ret i64 %missing}", "define i64 @f(i64 %x) {entry: br label %end\ndead: %v = or i64 %x, 0\nbr label %end\nend: ret i64 %v}"] {
        assert!(tv::validate_pure_i64(&src, &pin(input), Options::default()).is_err());
    }
}

#[test]
fn solver_exhaustion_and_invalid_budgets_cannot_be_equivalent() {
    let ir = emitted(CLAMP);
    assert_eq!(
        tv::validate_pure_i64(
            CLAMP,
            &ir,
            Options {
                resource_limit: 1,
                ..Options::default()
            }
        )
        .unwrap()
        .outcome,
        Outcome::Inconclusive
    );
    for options in [
        Options {
            timeout_ms: 0,
            ..Options::default()
        },
        Options {
            resource_limit: 0,
            ..Options::default()
        },
        Options {
            timeout_ms: 30_001,
            ..Options::default()
        },
    ] {
        assert!(tv::validate_pure_i64(CLAMP, &ir, options).is_err());
    }
}

#[test]
fn input_and_instruction_budgets_fail_closed() {
    let src = source("x: i64", "x");
    assert!(tv::validate_pure_i64(
        &format!("//{}\n{src}", "x".repeat(65_536)),
        &function("ret i64 %x"),
        Options::default()
    )
    .is_err());
    assert!(tv::validate_pure_i64(
        &src,
        &format!(";{}\n{}", "x".repeat(65_536), function("ret i64 %x")),
        Options::default()
    )
    .is_err());
    assert!(tv::validate_pure_i64(
        &source("x: i64", &format!("{}x{}", "(".repeat(49), ")".repeat(49))),
        &function("ret i64 %x"),
        Options::default()
    )
    .is_err());
    let params = (0..17)
        .map(|i| format!("x{i}: i64"))
        .collect::<Vec<_>>()
        .join(",");
    assert!(tv::validate_pure_i64(
        &source(&params, "0"),
        &function("ret i64 %x"),
        Options::default()
    )
    .is_err());
    let mut body = String::new();
    for i in 0..1024 {
        body += &format!("%v{i} = or i64 %x, 0\n");
    }
    body += "ret i64 %x";
    assert!(tv::validate_pure_i64(&src, &function(&body), Options::default()).is_err());
}

#[test]
fn block_and_path_expansion_budgets_fail_closed() {
    let src = source("x: i64", "x");
    let mut body = String::from("entry: br label %b0\n");
    for i in 0..64 {
        body += &format!("b{i}: br label %b{}\n", i + 1);
    }
    body += "b64: ret i64 %x";
    assert!(tv::validate_pure_i64(&src, &function(&body), Options::default()).is_err());
    let mut body = String::from("entry: %c = icmp eq i64 %x, 0\nbr label %b0\n");
    for i in 0..9 {
        body += &format!(
            "b{i}: br i1 %c, label %a{i}, label %z{i}\na{i}: br label %b{}\nz{i}: br label %b{}\n",
            i + 1,
            i + 1
        );
    }
    body += "b9: ret i64 %x";
    assert_eq!(
        tv::validate_pure_i64(&src, &function(&body), Options::default())
            .unwrap_err()
            .0,
        "LLVM path expansion limit"
    );
}

#[test]
fn source_literals_comments_and_ir_diagnostics_are_not_echoed() {
    let sentinel = "FAKE_PRIVATE_SOURCE_SENTINEL";
    let src = format!("// {sentinel}\n{}", source("x: i64", "x"));
    let ir = function("ret i64 %x");
    assert!(!format!("{:?}", validate(&src, &ir)).contains(sentinel));
    let error =
        tv::validate_pure_i64(&src, &format!("{sentinel}{ir}"), Options::default()).unwrap_err();
    assert!(!format!("{error:?} {error}").contains(sentinel));
    let literal = "1234567890123456789";
    let src = source("x: i64", literal);
    let ir = function(&format!("ret i64 {literal}"));
    assert!(!format!("{:?}", validate(&src, &ir)).contains(literal));
}

fn native_source(body: &str) -> String {
    format!("atom main() -> i64 requires: true; ensures: true; body: {{ {body} }};")
}

fn native_ir(body: &str) -> String {
    pin(&format!(
        "define i64 @__mumei_user_main() {{ {body} }}\n\
         define i32 @main(i32 %argc, ptr %argv) {{\n\
         %result = call i64 @__mumei_user_main()\n\
         %exit_code = trunc i64 %result to i32\n\
         ret i32 %exit_code\n}}"
    ))
}

fn native_validate(src: &str, ir: &str) -> tv::ValidationReport {
    tv::validate_native_main(src, ir, Options::default()).unwrap()
}

#[test]
fn native_emitter_bodies_and_truncating_wrapper_match_without_rewriting() {
    for body in [
        "0",
        "7",
        "2147483648",
        "4294967296",
        "9223372036854775807",
        "if 1 < 2 { if false { 99 } else { 7 } } else { 42 }",
        "if (1 < 2) && (7 != 8) { 7 } else { 99 }",
        "if (1 > 2) || (7 == 8) { 99 } else { 7 }",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(
        ["<", "<=", ">", ">=", "==", "!="]
            .into_iter()
            .map(|op| format!("if 7 {op} 8 {{ 7 }} else {{ 8 }}")),
    ) {
        let src = native_source(&body);
        let Item::Atom(atom) = parse_module_checked(&src).unwrap().remove(0) else {
            panic!("atom")
        };
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("native.ll");
        mumei_emit_llvm::binary::compile_atoms_to_binary_ll(
            &[lower_atom_to_hir(&atom)],
            &ModuleEnv::default(),
            &[],
            &path,
        )
        .unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let ir = pin(&raw);
        let report = native_validate(&src, &ir);
        assert_eq!(report.outcome, Outcome::EquivalentWithinProfile, "{body}");
        assert_eq!(report.profile, tv::NATIVE_PROFILE);
        assert_eq!(
            report.source_sha256,
            mumei_core::native_bundle::sha256(src.as_bytes())
        );
        assert_eq!(
            report.llvm_sha256,
            mumei_core::native_bundle::sha256(ir.as_bytes())
        );
        assert_eq!(
            report.input_domain,
            "zero_argument_main_i64_body_not_process_exit_status"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
        assert!(tv::validate_pure_i64(&src, &ir, Options::default()).is_err());
    }
}

#[test]
fn native_full_i64_body_must_match_even_when_low_32_bits_are_equal() {
    let src = native_source("7");
    for value in ["8", "4294967303", "9223372036854775807"] {
        assert_eq!(
            native_validate(&src, &native_ir(&format!("ret i64 {value}"))).outcome,
            Outcome::Different
        );
    }
    let ir = native_ir("%same = or i64 7, 0\nret i64 %same");
    assert_eq!(
        native_validate(&src, &ir).outcome,
        Outcome::EquivalentWithinProfile
    );
}

#[test]
fn native_snapshot_profile_is_separate_and_pins_exact_target_and_layout() {
    let src = native_source("7");
    let ir = native_ir("ret i64 7");
    let pc = ir.replace(tv::TARGET, tv::NATIVE_TARGET);
    assert_eq!(native_validate(&src, &pc).target, tv::NATIVE_TARGET);
    for bad in [
        ir.replace(tv::TARGET, "x86_64-pc-windows-msvc"),
        ir.replace(tv::TARGET, "x86_64-unknown-linux-musl"),
        ir.replace(tv::DATA_LAYOUT, "e-i64:64"),
        ir.replace(&format!("target triple = \"{}\"", tv::TARGET), ""),
        ir.replace("__mumei_user_main", "other"),
    ] {
        assert!(tv::validate_native_main(&src, &bad, Options::default()).is_err());
    }
    for bad in [
        source("", "7"),
        source("x: i64", "x"),
        native_source("7 / 1"),
    ] {
        assert!(tv::validate_native_main(&bad, &ir, Options::default()).is_err());
    }
    assert!(tv::validate_native_main(&src, &function("ret i64 7"), Options::default()).is_err());
}

#[test]
fn native_wrapper_must_be_only_a_direct_call_truncation_and_return() {
    let src = native_source("7");
    let ir = native_ir("ret i64 7");
    for (from, to) in [
        ("ret i32 %exit_code", "ret i32 7"),
        ("ret i32 %exit_code", "ret i32 %argc"),
        ("trunc i64 %result", "trunc i64 7"),
        ("trunc i64 %result to i32", "add i32 0, 7"),
        (
            "%result = call",
            "%unused = call i64 @__mumei_user_main()\n%result = call",
        ),
        ("%result = call", "%result = tail call"),
        ("call i64", "call noundef i64"),
        (
            "call i64 @__mumei_user_main()",
            "call i64 @__mumei_user_main() nounwind",
        ),
        (
            "call i64 @__mumei_user_main()",
            "call i64 @__mumei_user_main() [ \"deopt\"(i32 1) ]",
        ),
        ("ptr %argv", "ptr addrspace(1) %argv"),
        ("i32 %argc", "i32 noundef %argc"),
        ("define i32 @main", "define i32 @main_wrong"),
        ("define i32 @main", "define fastcc i32 @main"),
        ("define i32 @main", "define weak i32 @main"),
        (
            "define i64 @__mumei_user_main",
            "define weak i64 @__mumei_user_main",
        ),
        ("ptr %argv)", "ptr %argv, ...)"),
        (
            "%exit_code = trunc",
            "br label %next\nnext: %exit_code = trunc",
        ),
        ("ret i32 %exit_code", "ret i32 %exit_code, !private !0"),
    ] {
        assert_ne!(ir.replace(from, to), ir, "mutation {from}");
        assert!(
            tv::validate_native_main(&src, &ir.replace(from, to), Options::default()).is_err(),
            "{from} -> {to}"
        );
    }
}

#[test]
fn native_module_cannot_hide_extra_functions_globals_metadata_or_instructions() {
    let src = native_source("7");
    let ir = native_ir("ret i64 7");
    for extra in [
        "declare void @external()",
        "define i64 @other() { ret i64 7 }",
        "@global = global i64 7",
        "@alias = alias i64 (), ptr @__mumei_user_main",
        "module asm \"FAKE_PRIVATE_IR_ONLY\"",
        "!private = !{!0}\n!0 = !{i64 7}",
    ] {
        assert!(
            tv::validate_native_main(&src, &format!("{ir}\n{extra}"), Options::default()).is_err()
        );
    }
    for body in [
        "ret i64 poison",
        "ret i64 undef",
        "%p = alloca i64\nret i64 7",
        "%x = add nsw i64 6, 1\nret i64 %x",
        "entry: br label %entry",
        "ret i64 7, !private !0\n}\n!0 = !{i64 7}\ndefine i64 @extra() { ret i64 7",
    ] {
        assert!(tv::validate_native_main(&src, &native_ir(body), Options::default()).is_err());
    }
    for from_to in [
        ("define i64", "define fastcc i64"),
        ("define i64", "define noundef i64"),
        ("@__mumei_user_main() {", "@__mumei_user_main() nounwind {"),
    ] {
        assert!(tv::validate_native_main(
            &src,
            &ir.replace(from_to.0, from_to.1),
            Options::default()
        )
        .is_err());
    }
}

#[test]
fn native_budgets_and_private_parser_diagnostics_fail_closed() {
    let src = native_source("7");
    let ir = native_ir("ret i64 7");
    for options in [
        Options {
            timeout_ms: 0,
            ..Options::default()
        },
        Options {
            timeout_ms: 30_001,
            ..Options::default()
        },
        Options {
            resource_limit: 0,
            ..Options::default()
        },
    ] {
        assert!(tv::validate_native_main(&src, &ir, options).is_err());
    }
    for bad in [
        format!("{ir};{}", "x".repeat(65_536)),
        "[".repeat(49),
        "FAKE_PRIVATE_IR_ONLY".into(),
    ] {
        let error = tv::validate_native_main(&src, &bad, Options::default()).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("FAKE_PRIVATE_IR_ONLY"));
    }
    let report = tv::validate_native_main(
        &src,
        &ir,
        Options {
            resource_limit: 1,
            ..Options::default()
        },
    )
    .unwrap();
    assert_eq!(report.outcome, Outcome::Inconclusive);
}

#[test]
fn native_empty_operand_bundles_are_not_plain_direct_calls() {
    let src = native_source("7");
    let ir = native_ir("ret i64 7");
    for bundle in ["\"custom\"()", "\"deopt\"()", "\"custom\"(i32 1)"] {
        let changed = ir.replace(
            "call i64 @__mumei_user_main()",
            &format!("call i64 @__mumei_user_main() [ {bundle} ]"),
        );
        assert!(
            tv::validate_native_main(&src, &changed, Options::default()).is_err(),
            "{bundle}"
        );
    }
    let renamed = ir.replace("%result", "%\"call name []\"");
    assert_eq!(
        native_validate(&src, &renamed).outcome,
        Outcome::EquivalentWithinProfile
    );
}
