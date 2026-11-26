use std::path::PathBuf;
use std::process::{Command, Output};

fn verify_source(name: &str, source: &str, bitvec: bool) -> (PathBuf, Output) {
    let dir = std::env::temp_dir().join(format!(
        "mumei_refinement_operand_{}_{}_{}",
        std::process::id(),
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let fixture = dir.join(format!("{name}.mm"));
    std::fs::write(&fixture, source).expect("write fixture");
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.arg("verify").arg("--report-dir").arg(&dir);
    if bitvec {
        command.arg("--bitvec-i64");
    }
    let output = command
        .arg(&fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify");
    (dir, output)
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn refinements_use_their_declared_operand_in_both_integer_modes() {
    let cases = [
        (
            "parenthesized_bitwise",
            "type T = u64 where (v | 0) <= 10;\n\
             atom f(x: u64) -> T requires: true; ensures: true; body: x;",
            false,
        ),
        (
            "captured_range",
            "type T = u64 where (v | 0) <= 10;\n\
             atom f(v: u64, x: u64) -> T requires: v <= 10; ensures: true; body: x;",
            false,
        ),
        (
            "range",
            "type T = i64 where 0 <= v && v <= 10;\n\
             atom f(x: i64) -> T requires: true; ensures: true; body: x;",
            false,
        ),
        (
            "parenthesized_positive",
            "type T = i64 where (v) <= 10;\n\
             atom g(t: T) -> i64 requires: true; ensures: result <= 10; body: t;",
            true,
        ),
        (
            "captured_range_positive",
            "type T = i64 where 0 <= v && v <= 10;\n\
             atom f(v: i64, x: i64) -> T requires: v == 5; ensures: true; body: x;",
            false,
        ),
    ];

    for (name, source, should_verify) in cases {
        for bitvec in [false, true] {
            let (dir, output) = verify_source(name, source, bitvec);
            let text = output_text(&output);
            assert_eq!(
                output.status.success(),
                should_verify,
                "{name} (bitvec={bitvec}) had unexpected result:\n{text}"
            );
            std::fs::remove_dir_all(dir).expect("remove fixture directory");
        }
    }
}

#[test]
fn free_refinement_identifiers_are_rejected_in_both_integer_modes() {
    for (name, source) in [
        (
            "free_identifier_return",
            "type T = i64 where v <= n;\n\
             atom f(n: i64, x: i64) -> T requires: n >= 100 && x == 50; \
             ensures: true; body: x;",
        ),
        (
            "free_identifier_parameter",
            "type T = i64 where v <= n;\n\
             atom g(n: i64, t: T) -> i64 requires: n == 10; \
             ensures: result <= 10; body: t;",
        ),
    ] {
        for bitvec in [false, true] {
            let (dir, output) = verify_source(name, source, bitvec);
            let text = output_text(&output);
            assert!(
                !output.status.success(),
                "{name} (bitvec={bitvec}) unexpectedly verified:\n{text}"
            );
            assert!(
                text.contains(
                    "Refinement predicate of type 'T' references free identifier 'n'; only the operand 'v' may appear"
                ),
                "{name} (bitvec={bitvec}) had wrong diagnostic:\n{text}"
            );
            std::fs::remove_dir_all(dir).expect("remove fixture directory");
        }
    }
}

#[test]
fn bare_enum_names_are_rejected_and_qualified_enum_paths_remain_allowed() {
    let cases = [
        (
            "bare_enum_name",
            "enum E { A, B }\n\
             type T = i64 where v <= E;\n\
             atom f(E: i64, x: i64) -> T requires: E >= 100 && x == 50; \
             ensures: result == 50; body: x;",
            false,
        ),
        (
            "qualified_enum_path",
            "enum E { A, B }\n\
             type T = i64 where v >= 0 && E::A == E::A;\n\
             atom f(x: i64) -> T requires: x >= 0; ensures: result == x; body: x;",
            true,
        ),
    ];

    for (name, source, should_verify) in cases {
        for bitvec in [false, true] {
            let (dir, output) = verify_source(name, source, bitvec);
            let text = output_text(&output);
            assert_eq!(
                output.status.success(),
                should_verify,
                "{name} (bitvec={bitvec}) had unexpected result:\n{text}"
            );
            if !should_verify {
                assert!(
                    text.contains(
                        "Refinement predicate of type 'T' references free identifier 'E'; only the operand 'v' may appear"
                    ),
                    "{name} (bitvec={bitvec}) had wrong diagnostic:\n{text}"
                );
            }
            std::fs::remove_dir_all(dir).expect("remove fixture directory");
        }
    }
}

#[test]
fn quantifier_and_named_refinement_operands_remain_supported() {
    for bitvec in [false, true] {
        for (name, source) in [
            (
                "quantifier_binder",
                "type T = i64 where forall(i, 0, 1, i >= 0);\n\
                 atom f(x: i64) -> T requires: true; ensures: result == x; body: x;",
            ),
            (
                "quantifier_bound_before_operand",
                "type T = i64 where forall(i, 0, v, i >= 0);\n\
                 atom f(x: i64) -> T requires: x >= 0; \
                 ensures: result == x; body: x;",
            ),
            (
                "named_operands",
                "type Req = i64 where req_handle > 0;\n\
                 type Server = i64 where server_handle > 0;\n\
                 atom f(req: Req, server: Server) -> i64 requires: true; \
                 ensures: result > 0; body: req;",
            ),
        ] {
            let (dir, output) = verify_source(name, source, bitvec);
            let text = output_text(&output);
            assert!(
                output.status.success(),
                "{name} (bitvec={bitvec}) failed:\n{text}"
            );
            std::fs::remove_dir_all(dir).expect("remove fixture directory");
        }
    }
}
