//! Regression tests for the consume-spelling unification: the signature
//! marker `atom f(consume x: T)` and the `consume x;` clause mean the same
//! thing — the atom takes ownership of the parameter. Before the unification,
//! signature-`consume` bypassed the linearity machinery, so a caller could
//! reuse an argument after calling a signature-`consume` callee without any
//! violation. Both spellings now fail identically, and declaring `consume`
//! twice for the same parameter is rejected.

use std::process::{Command, Output};

fn verify_source(tag: &str, source: &str) -> (Output, String) {
    let dir = std::env::temp_dir().join(format!(
        "mumei_consume_unify_{}_{}_{}",
        std::process::id(),
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let fixture = dir.join("fixture.mm");
    let report_dir = dir.join("reports");
    std::fs::create_dir_all(&report_dir).expect("create report directory");
    std::fs::write(&fixture, source).expect("write fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--report-dir")
        .arg(&report_dir)
        .arg("--disable-spurious-detection")
        .arg(&fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(dir).expect("remove fixture directory");
    (output, text)
}

const CALLEE_SIGNATURE_CONSUME: &str = "atom consumer(consume x: i64)\n    requires: x >= 0;\n    ensures: result >= 0;\n    body: x;\n";

const CALLEE_CLAUSE_CONSUME: &str = "atom consumer(x: i64)\n    requires: x >= 0;\n    consume x;\n    ensures: result >= 0;\n    body: x;\n";

const REUSING_CALLER: &str = "atom reuse_arg(x: i64)\n    requires: x >= 0;\n    ensures: result >= 0;\n    body: {\n        let y = consumer(x);\n        x\n    };\n";

const NON_REUSING_CALLER: &str = "atom take_arg(x: i64)\n    requires: x >= 0;\n    ensures: result >= 0;\n    body: {\n        let y = consumer(x);\n        y\n    };\n";

#[test]
fn signature_consume_arg_reuse_fails() {
    let (output, text) = verify_source(
        "sig_reuse",
        &format!("{CALLEE_SIGNATURE_CONSUME}\n{REUSING_CALLER}"),
    );
    assert!(
        !output.status.success(),
        "reusing an argument after a signature-consume call must fail, got:\n{text}"
    );
    assert!(
        text.contains("used after being moved") || text.contains("Linearity"),
        "expected a move/linearity violation, got:\n{text}"
    );
}

#[test]
fn clause_consume_arg_reuse_fails_identically() {
    // Parity guard: the `consume x;` clause spelling already rejected this;
    // it must keep failing after the unification.
    let (output, text) = verify_source(
        "clause_reuse",
        &format!("{CALLEE_CLAUSE_CONSUME}\n{REUSING_CALLER}"),
    );
    assert!(
        !output.status.success(),
        "reusing an argument after a clause-consume call must fail, got:\n{text}"
    );
}

#[test]
fn signature_consume_call_without_reuse_verifies() {
    let (output, text) = verify_source(
        "sig_no_reuse",
        &format!("{CALLEE_SIGNATURE_CONSUME}\n{NON_REUSING_CALLER}"),
    );
    assert!(
        output.status.success(),
        "moving the argument into a signature-consume call must verify, got:\n{text}"
    );
}

#[test]
fn clause_consume_call_without_reuse_verifies() {
    let (output, text) = verify_source(
        "clause_no_reuse",
        &format!("{CALLEE_CLAUSE_CONSUME}\n{NON_REUSING_CALLER}"),
    );
    assert!(
        output.status.success(),
        "moving the argument into a clause-consume call must verify, got:\n{text}"
    );
}

#[test]
fn double_consume_declaration_is_rejected() {
    // `consume x` in the signature AND `consume x;` as a clause declares the
    // same parameter twice — that is a type error, not a double-consume.
    let (output, text) = verify_source(
        "double_declare",
        r#"
atom consumer(consume x: i64)
    requires: x >= 0;
    consume x;
    ensures: result >= 0;
    body: x;
"#,
    );
    assert!(
        !output.status.success(),
        "declaring consume twice for the same parameter must fail, got:\n{text}"
    );
    assert!(
        text.contains("declared consume twice"),
        "expected the double-declaration error, got:\n{text}"
    );
}

#[test]
fn trusted_atom_double_consume_declaration_is_rejected() {
    // Trusted atoms return before the executor's linearity registration, so
    // the double-declaration check must run before the trust-level early
    // return — a `consume x` written twice must be rejected there too.
    let (output, text) = verify_source(
        "trusted_double",
        r#"
trusted atom consumer(consume x: i64) -> i64
requires: x >= 0;
ensures: result >= 0;
consume x;
body: x;
"#,
    );
    assert!(
        !output.status.success(),
        "a trusted atom declaring consume twice must fail, got:\n{text}"
    );
    assert!(
        text.contains("declared consume twice"),
        "expected the double-declaration error, got:\n{text}"
    );
}

#[test]
fn signature_consume_param_forwarded_to_consumer_verifies() {
    // Ownership transfer chains: `forward` takes ownership of `y` and moves it
    // into `consumer` — a legitimate transfer, not a double-free. The
    // post-body marking must not re-consume a parameter the body already
    // handed off.
    let (output, text) = verify_source(
        "sig_forward",
        &format!(
            "{CALLEE_SIGNATURE_CONSUME}\n\
             atom forward(consume y: i64)\n    requires: y >= 0;\n    ensures: result >= 0;\n    body: {{\n        let z = consumer(y);\n        z\n    }};\n"
        ),
    );
    assert!(
        output.status.success(),
        "forwarding a signature-consume parameter to a consuming callee must verify, got:\n{text}"
    );
}

#[test]
fn clause_consume_param_forwarded_to_consumer_verifies() {
    let (output, text) = verify_source(
        "clause_forward",
        &format!(
            "{CALLEE_CLAUSE_CONSUME}\n\
             atom forward(y: i64)\n    requires: y >= 0;\n    ensures: result >= 0;\n    consume y;\n    body: {{\n        let z = consumer(y);\n        z\n    }};\n"
        ),
    );
    assert!(
        output.status.success(),
        "forwarding a clause-consume parameter to a consuming callee must verify, got:\n{text}"
    );
}
