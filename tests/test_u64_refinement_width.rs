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

#[test]
fn bitwise_result_rejects_u64_wraparound() {
    let output = run_verify("bitwise_result");
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "a bitwise u64 result outside the postcondition must fail: {text}"
    );
    assert!(
        text.contains("Postcondition (ensures) is not satisfied"),
        "{text}"
    );
}

#[test]
fn shift_result_preserves_left_u64_width() {
    let output = run_verify("shift_result");
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "a shifted u64 result outside the postcondition must fail: {text}"
    );
    assert!(
        text.contains("Postcondition (ensures) is not satisfied"),
        "{text}"
    );
}

#[test]
fn shift_width_uses_right_u64_operand() {
    let output = run_verify("shift_right");
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "a shift using a right u64 operand must retain its unsigned width: {text}"
    );
    assert!(
        text.contains("ensures clause 1 is inconsistent with requires"),
        "{text}"
    );
}

#[test]
fn block_tail_preserves_u64_width() {
    let output = run_verify("block");
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "a u64 block tail outside the postcondition must fail: {text}"
    );
    assert!(
        text.contains("ensures clause 1 is inconsistent with requires"),
        "{text}"
    );
}

#[test]
fn branch_local_let_shadow_uses_signed_comparison() {
    let output = run_verify("shadow");
    assert!(
        output.status.success(),
        "a local let shadowing a u64 parameter must use its inferred signed width\n{}",
        output_text(&output)
    );
}

#[test]
fn refined_u64_predicate_rejects_operand_rebinding() {
    let output = run_verify("rebind");
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "a refined u64 predicate must reject operand rebinding: {text}"
    );
    assert!(
        text.contains("Refinement predicate for 'Small' rebinds its operand 'v'"),
        "{text}"
    );
}

#[test]
fn signed_i64_refinement_with_operand_rebinding_remains_usable() {
    let output = run_verify("i64_rebind_control");
    assert!(
        output.status.success(),
        "a signed i64 refinement should remain usable\n{}",
        output_text(&output)
    );
}

#[test]
fn refined_u8_predicate_with_operand_rebinding_remains_usable() {
    let output = run_verify("u8_rebind_control");
    assert!(
        output.status.success(),
        "a u8 refinement with a same-name quantifier must remain usable\n{}",
        output_text(&output)
    );
}

#[test]
fn branch_local_struct_field_comparison_preserves_u64_width() {
    let output = run_verify("branch_struct_field");
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "a branch-local u64 struct field comparison must reject the signed reading: {text}"
    );
    assert!(
        text.contains("Postcondition (ensures) is not satisfied"),
        "{text}"
    );
}
