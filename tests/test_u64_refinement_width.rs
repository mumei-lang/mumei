use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(format!("test_u64_refinement_width_{name}.mm"))
}

fn run_verify(name: &str) -> Output {
    let dir = std::env::temp_dir().join(format!(
        "mumei_u64_refinement_width_{}_{}",
        std::process::id(),
        name
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = fixture(name);
    let copied = dir.join(source.file_name().expect("fixture name"));
    std::fs::copy(source, &copied).expect("copy fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--report-dir")
        .arg(&dir)
        .arg(&copied)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify");
    std::fs::remove_dir_all(dir).ok();
    output
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn return_refinement_rejects_u64_wraparound() {
    let output = run_verify("return");
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "a wrapped u64 return outside the refinement must fail: {text}"
    );
    assert!(
        text.contains("Return type 'Small' refinement is not satisfied"),
        "{text}"
    );
}

#[test]
fn call_argument_refinement_rejects_u64_wraparound() {
    let output = run_verify("call");
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "a wrapped u64 call argument outside the refinement must fail: {text}"
    );
    assert!(text.contains("refinement on parameter 'a'"), "{text}");
}

#[test]
fn parameter_refinement_uses_unsigned_u64_comparison() {
    let output = run_verify("param");
    assert!(
        output.status.success(),
        "a u64 parameter refinement must constrain the unsigned range\n{}",
        output_text(&output)
    );
}

#[test]
fn u64_refinement_includes_values_above_i64_max() {
    let output = run_verify("big");
    assert!(
        output.status.success(),
        "a u64 refinement above i64::MAX must verify\n{}",
        output_text(&output)
    );
}

#[test]
fn usize_refinement_uses_u64_width() {
    let output = run_verify("usize");
    assert!(
        output.status.success(),
        "a usize refinement must use its resolved u64 width\n{}",
        output_text(&output)
    );
}

#[test]
fn struct_field_refinement_uses_unsigned_u64_comparison() {
    let output = run_verify("struct");
    assert!(
        output.status.success(),
        "a refined u64 struct field must constrain the unsigned range\n{}",
        output_text(&output)
    );
}
