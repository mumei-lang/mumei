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

fn combined(output: &std::process::Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn explicit_type_arg_call_resolves_monomorphized_atom() {
    // `id<i64>(3)` instantiates `id<T>`; `ensures: result == 3` only proves
    // because the callee's `result == x` contract flows through the call.
    let output = mumei_verify("tests/test_generic_call.mm");
    assert!(
        output.status.success(),
        "`id<i64>(3)` must verify `result == 3` via the monomorphized atom:\n{}",
        combined(&output)
    );
}

#[test]
fn explicit_type_arg_call_unknown_callee_fails() {
    let output = mumei_verify("tests/test_generic_call_negative.mm");
    let text = combined(&output);
    assert!(
        !output.status.success(),
        "`mystery<i64>(3)` must fail closed, not verify:\n{text}"
    );
    assert!(
        text.contains("Unknown function: mystery<i64>"),
        "expected an unresolved-callee error, got:\n{text}"
    );
}

#[test]
fn explicit_type_arg_call_effect_instantiation() {
    // apply<T: Comparable, E: Effect> + `atom_ref` argument: the call
    // `apply<i64, Network>(42, atom_ref(net_fn))` must monomorphize and the
    // Network effect must propagate through `with E`.
    let output = mumei_verify("tests/effect_polymorphism_mixed.mm");
    assert!(
        output.status.success(),
        "apply<i64, Network> call must verify:\n{}",
        combined(&output)
    );
}

#[test]
fn explicit_type_arg_call_effect_violation_detected() {
    // `pipe<Network>` inside an atom declaring only FileWrite must fail
    // with a real effect-propagation violation.
    let output = mumei_verify("tests/effect_polymorphism_violation.mm");
    let text = combined(&output);
    assert!(
        !output.status.success(),
        "effect violation must fail verification:\n{text}"
    );
    assert!(
        text.contains("pipe<Network>"),
        "expected the violation to name pipe<Network>, got:\n{text}"
    );
}
