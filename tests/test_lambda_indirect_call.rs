use std::process::Command;

fn mumei_verify(file: &str) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_mumei");
    Command::new(bin)
        .arg("verify")
        .arg(file)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify {file}: {err}"))
}

#[test]
fn let_bound_lambdas_apply_at_call_sites() {
    let output = mumei_verify("tests/test_lambda_indirect_call.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "`let f = |a| a + 1; f(3)` and `call(f, 3)` must inline the lambda body —\n\
         direct calls, multi-arg, captures, shadowed params, aliasing, inline\n\
         literals, higher-order lambdas, and branch/loop/match scoping:\n{combined}"
    );
}

#[test]
fn lambda_basic_callref_still_verifies() {
    let output = mumei_verify("tests/test_lambda_basic.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "call(f, x) on a let-bound lambda must verify:\n{combined}"
    );
}

#[test]
fn indirect_lambda_calls_fail_closed() {
    let output = mumei_verify("tests/test_lambda_indirect_call_negative.mm");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "wrong postconditions, arity mismatches, rebound names, recursive\n\
         lambdas, and divergent branch bindings must all FAIL:\n{combined}"
    );
    assert!(
        combined.contains("Counterexample validated for atom 'call_lambda_wrong_post'"),
        "a wrong `ensures` on f(3) is a genuine counterexample, not spurious:\n{combined}"
    );
}
