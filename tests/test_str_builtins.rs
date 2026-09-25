use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn mumei_verify_uncached(file: &str) -> (bool, String) {
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
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(&dst)
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
fn invalid_str_builtins_are_rejected() {
    let (ok, out) = mumei_verify_uncached("tests/negative/test_str_builtins_bad.mm");
    assert!(!ok, "invalid Str builtin fixture must fail:\n{out}");
    assert!(
        out.contains("bad_is_empty") || out.contains("bad_index_of"),
        "expected a named failing atom:\n{out}"
    );
    assert!(
        out.contains("Str") || out.contains("is_empty") || out.contains("index_of"),
        "expected a builtin/type diagnostic:\n{out}"
    );
}
