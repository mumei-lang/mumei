use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn mumei_verify_uncached_with_args(file: &str, args: &[&str]) -> (bool, String) {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "mumei_str_builtins_{}_{}",
        std::process::id(),
        nonce
    ));
    fs::create_dir_all(&dir).expect("fixture dir");
    let dst = dir.join("main.mm");
    fs::copy(&src, &dst).expect("copy fixture");
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.arg("verify");
    command.args(args);
    command.arg(&dst);
    let output = command
        .current_dir(&dir)
        .output()
        .expect("run mumei verify");
    let _ = fs::remove_dir_all(&dir);
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), combined)
}

fn mumei_verify_uncached(file: &str) -> (bool, String) {
    mumei_verify_uncached_with_args(file, &[])
}

#[test]
fn str_builtins_verify() {
    let (ok, out) = mumei_verify_uncached("tests/test_str_builtins.mm");
    assert!(ok, "Str builtin fixture must verify:\n{out}");
    for atom in [
        "string_is_empty",
        "string_index_of",
        "string_substr",
        "string_char_at",
        "string_contains",
    ] {
        assert!(
            out.contains(&format!("'{atom}': verified")),
            "expected {atom} to verify:\n{out}"
        );
    }
}

#[test]
fn user_atoms_shadow_string_builtin_names() {
    let (ok, out) = mumei_verify_uncached("tests/test_str_builtins_list_shadow.mm");
    assert!(ok, "user atoms must shadow Str builtin names:\n{out}");
    for atom in ["unqualified_list_is_empty", "qualified_list_is_empty"] {
        assert!(
            out.contains(&format!("'{atom}': verified")),
            "expected {atom} to verify:\n{out}"
        );
    }
}

#[test]
fn str_builtins_verify_in_bitvec_mode() {
    let (ok, out) =
        mumei_verify_uncached_with_args("tests/test_str_builtins_bitvec.mm", &["--bitvec-i64"]);
    assert!(
        ok,
        "Str builtin fixture must verify in bit-vector mode:\n{out}"
    );
}

#[test]
fn str_substr_len_fails_closed_in_bitvec_mode() {
    let (ok, out) = mumei_verify_uncached_with_args(
        "tests/negative/test_str_builtins_bitvec_unknown.mm",
        &["--bitvec-i64"],
    );
    assert!(!ok, "mixed Str/bit-vector goal must fail closed:\n{out}");
    assert!(
        out.contains("unknown") || out.contains("Lean"),
        "expected an unknown or Lean-escalation diagnostic:\n{out}"
    );
}

#[test]
fn string_counterexample_replays_lengths_and_builtins() {
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--json")
        .arg("--enable-spurious-detection")
        .arg("tests/negative/test_str_len_cex.mm")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run string counterexample fixture");
    assert!(!output.status.success(), "fixture must fail verification");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let payload: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("verify --json emits one JSON payload");
    let counterexample = payload["semantic_feedback"]["reconstruction_loss"]["counter_example"]
        .as_object()
        .expect("loss-vector JSON should contain reconstruction counterexample");
    let string = counterexample["s"]
        .as_str()
        .expect("string parameter should be serialized as a string");
    let len_s = counterexample["len_s"]
        .as_i64()
        .expect("string length should be serialized as an integer");
    assert_eq!(len_s, string.chars().count() as i64);
    assert_eq!(
        payload["semantic_feedback"]["counterexample_validation_status"],
        serde_json::json!("validated")
    );
}

#[test]
fn false_str_builtin_ensures_are_rejected() {
    let (ok, out) = mumei_verify_uncached("tests/negative/test_str_builtins_false_ensures.mm");
    assert!(!ok, "false Str builtin ensures must fail:\n{out}");
    assert!(
        out.contains("bad_is_empty"),
        "expected bad_is_empty to fail:\n{out}"
    );
    assert!(
        !out.contains("syntax"),
        "false ensures should reach verification rather than fail parsing:\n{out}"
    );
}

#[test]
fn str_builtin_type_errors_are_rejected() {
    let (ok, out) = mumei_verify_uncached("tests/negative/test_str_builtins_type_error.mm");
    assert!(!ok, "wrong-type Str builtin use must fail:\n{out}");
    assert!(
        out.contains("index_of() expects a Str"),
        "expected index_of type diagnostic:\n{out}"
    );
}
