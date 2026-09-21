use std::process::Command;

fn write_fixture(name: &str, source: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mumei_lambda_codegen_test_{}_{}",
        name,
        std::process::id()
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale lambda fixture dir");
    }
    std::fs::create_dir_all(&dir).expect("create lambda fixture dir");
    let path = dir.join("main.mm");
    std::fs::write(&path, source).expect("write lambda fixture");
    path
}

fn mumei_run(fixture: &std::path::Path) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_mumei");
    Command::new(bin)
        .arg("run")
        .arg(fixture)
        .output()
        .unwrap_or_else(|err| panic!("failed to run lambda fixture: {err}"))
}

// Native codegen for indirect calls through let-bound lambdas (`f(args)` and
// `call(f, args)`): each `let f = |…| …` lifts to a module-level private
// function taking the captured values as leading params, and the tracked
// `@lam:` marker resolves `f` at call sites. Verification of the same
// programs is covered by tests/test_lambda_indirect_call.rs (#615); these
// tests prove the emitted native code computes the same semantics.

#[test]
fn lambda_basic_call_executes() {
    let fixture = write_fixture(
        "basic",
        r#"
trusted atom main()
requires: true;
ensures: true;
body: {
    let f = |a: i64| a + 1;
    f(6)
};
"#,
    );
    let output = mumei_run(&fixture);
    assert_eq!(output.status.code(), Some(7), "f(6) must return 7");
}

#[test]
fn lambda_captures_and_multi_arg_execute() {
    let fixture = write_fixture(
        "captures",
        r#"
trusted atom main()
requires: true;
ensures: true;
body: {
    let k = 10;
    let f = |a: i64, b: i64| a * b + k;
    f(3, 4)
};
"#,
    );
    let output = mumei_run(&fixture);
    assert_eq!(output.status.code(), Some(22), "f(3,4) must return 22");
}

#[test]
fn lambda_callref_and_inline_literal_execute() {
    let fixture = write_fixture(
        "callref",
        r#"
trusted atom main()
requires: true;
ensures: true;
body: {
    let f = |a: i64| a + 1;
    call(|x: i64| x * 3, 7) + call(f, 2)
};
"#,
    );
    let output = mumei_run(&fixture);
    // call(|x| x*3, 7) = 21; call(f, 2) = 3; total 24
    assert_eq!(output.status.code(), Some(24));
}

#[test]
fn lambda_alias_and_transitive_captures_execute() {
    // `g` references `f` as a call target — not a free variable — so the
    // lifted `g` must take `f`'s own value captures (`k`) as extra params.
    // `k` is rebound to 20 before the call, so the call-site value flows in.
    let fixture = write_fixture(
        "transitive",
        r#"
trusted atom main()
requires: true;
ensures: true;
body: {
    let k = 5;
    let f = |x: i64| x + k;
    let g = |y: i64| f(y) * 10;
    let h = f;
    k = 20;
    g(1) - h(0) * 10
};
"#,
    );
    let output = mumei_run(&fixture);
    // g(1) = f(1)*10 = 21*10 = 210; h(0)*10 = 20*10 = 200; result = 10
    assert_eq!(output.status.code(), Some(10));
}

#[test]
fn lambda_array_capture_executes() {
    let fixture = write_fixture(
        "array",
        r#"
trusted atom main()
requires: true;
ensures: true;
body: {
    let arr = [10, 20, 30];
    let at = |i: i64| arr[i];
    at(1)
};
"#,
    );
    let output = mumei_run(&fixture);
    assert_eq!(
        output.status.code(),
        Some(20),
        "at(1) must return arr[1] = 20"
    );
}

#[test]
fn lambda_branch_scope_and_rebind_execute() {
    // `if` binds `h` only on the then side; post-merge the marker is dropped
    // (single-side bindings leak per the verifier's scope rule is NOT what
    // codegen does — both sides live in one function, so the marker merge
    // must mirror the verifier: keep only names bound on both sides).
    let fixture = write_fixture(
        "branch",
        r#"
trusted atom main()
requires: true;
ensures: true;
body: {
    let f = |x: i64| x + 100;
    let c = true;
    let r = if c { f(1) } else { 0 };
    f = |x: i64| x * 2;
    r + f(3)
};
"#,
    );
    let output = mumei_run(&fixture);
    // r = 101; rebound f(3) = 6; total 107
    assert_eq!(output.status.code(), Some(107));
}

#[test]
fn lambda_param_shadows_outer_lambda() {
    // `|f| …` param named f must shadow the outer lambda binding.
    let fixture = write_fixture(
        "shadow",
        r#"
trusted atom main()
requires: true;
ensures: true;
body: {
    let f = |x: i64| x + 1;
    let g = |f: i64| f + 100;
    g(5) + f(0)
};
"#,
    );
    let output = mumei_run(&fixture);
    // g(5) = 105; f(0) = 1; total 106
    assert_eq!(output.status.code(), Some(106));
}

#[test]
fn lambda_shadows_same_named_atom() {
    // `let inc = |a| a * 10` must shadow the atom `inc` — the verifier
    // resolves local lambdas before the atom table, so codegen must too;
    // otherwise it calls the atom and returns 2 where verification proved 10.
    let fixture = write_fixture(
        "atom_shadow",
        r#"
atom inc(a: i64) -> i64
requires: true;
ensures: result == a + 1;
body: { a + 1 };

trusted atom main()
requires: true;
ensures: true;
body: {
    let inc = |a: i64| a * 10;
    inc(1)
};
"#,
    );
    let output = mumei_run(&fixture);
    assert_eq!(
        output.status.code(),
        Some(10),
        "local lambda must shadow atom"
    );
}

#[test]
fn lambda_survives_while_loop() {
    let fixture = write_fixture(
        "while",
        r#"
trusted atom main()
requires: true;
ensures: true;
body: {
    let i = 0;
    let f = |x: i64| x + 100;
    while i < 3
    invariant: i >= 0 && i <= 3
    decreases: 3 - i
    {
        i = i + 1
    };
    f(1)
};
"#,
    );
    let output = mumei_run(&fixture);
    assert_eq!(output.status.code(), Some(101));
}
