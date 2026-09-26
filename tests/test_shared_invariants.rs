use std::process::Command;

fn run_verify(file: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(file)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap_or_else(|err| panic!("failed to verify {file}: {err}"))
}

fn combined(output: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn shared_state_positive_and_sibling_tasks_verify() {
    let output = run_verify("tests/positive/shared_invariant_counter.mm");
    assert!(
        output.status.success(),
        "shared resource fixture should verify:\n{}",
        combined(&output)
    );
}

#[test]
fn broken_invariant_is_a_hard_error() {
    let output = run_verify("tests/negative/shared_invariant_broken.mm");
    assert!(!output.status.success());
    assert!(combined(&output)
        .contains("shared invariant of resource 'counter' not re-established at unlock"));
}

#[test]
fn conditional_invariant_break_is_detected() {
    let output = run_verify("tests/negative/shared_invariant_conditional.mm");
    assert!(!output.status.success());
    assert!(combined(&output)
        .contains("shared invariant of resource 'counter' not re-established at unlock"));
}

#[test]
fn unlocked_and_shared_mode_writes_are_rejected() {
    let unlocked = run_verify("tests/negative/shared_state_unlocked.mm");
    assert!(!unlocked.status.success());
    assert!(combined(&unlocked)
        .contains("shared state 'counter.value' written outside 'acquire counter'"));

    let unlocked_read = run_verify("tests/negative/shared_state_unlocked_read.mm");
    assert!(!unlocked_read.status.success());
    assert!(combined(&unlocked_read)
        .contains("shared state 'counter.value' accessed outside 'acquire counter'"));

    let shared = run_verify("tests/negative/shared_mode_write.mm");
    assert!(!shared.status.success());
    assert!(combined(&shared)
        .contains("cannot write shared state 'counter.value' under mode: shared (read-only)"));
}

#[test]
fn initial_state_type_and_shadowing_checks_are_hard_errors() {
    let initial = run_verify("tests/negative/shared_invariant_initial.mm");
    assert!(!initial.status.success());
    assert!(combined(&initial).contains(
        "resource 'counter' invariant does not hold for the initial state (all fields zero)"
    ));

    let mismatch = run_verify("tests/negative/shared_state_type_mismatch.mm");
    assert!(!mismatch.status.success());
    assert!(combined(&mismatch).contains("shared state 'counter.ready' has type bool"));

    let shadowed = run_verify("tests/negative/shared_state_shadowing.mm");
    assert!(!shadowed.status.success());
    assert!(combined(&shadowed)
        .contains("identifier 'counter' shadows resource 'counter' with shared state"));
}

#[test]
fn separate_acquires_do_not_reuse_state_cells() {
    let output = run_verify("tests/negative/shared_state_reacquire.mm");
    assert!(!output.status.success());
}

#[test]
fn resource_codegen_emits_typed_state_globals() {
    let output_base =
        std::env::temp_dir().join(format!("mumei-shared-invariant-{}", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["build", "--emit", "llvm-ir", "-o"])
        .arg(&output_base)
        .arg("tests/positive/shared_invariant_counter.mm")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to build shared resource fixture");
    assert!(
        output.status.success(),
        "resource codegen should succeed:\n{}",
        combined(&output)
    );
    let ir = std::fs::read_to_string(output_base.with_file_name(format!(
        "{}_bump.ll",
        output_base.file_name().unwrap().to_string_lossy()
    )))
    .expect("resource LLVM IR should be written");
    assert!(ir.contains("@__mumei_res_counter_value"));
}

#[test]
fn resource_and_ownership_regressions_keep_their_status() {
    let consume_ref = run_verify("tests/negative/consume_ref_conflict.mm");
    assert!(!consume_ref.status.success());

    let resource = run_verify("tests/test_effect_resource_combo.mm");
    assert!(
        resource.status.success(),
        "existing resource fixture regressed:\n{}",
        combined(&resource)
    );
}
