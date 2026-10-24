use std::process::Command;

fn write_fixture(name: &str, source: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mumei_lit_{}_{}", name, std::process::id()));
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    let path = dir.join(format!("{name}.mm"));
    std::fs::write(&path, source).expect("write fixture");
    path
}

fn emit_atom_ir(name: &str, source: &str, atom: &str) -> String {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let fixture = write_fixture(name, source);
    let dir = fixture.parent().unwrap().to_path_buf();
    let output = Command::new(bin)
        .arg("build")
        .arg(&fixture)
        .arg("--emit")
        .arg("llvm-ir")
        .current_dir(&dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to build fixture {name}: {err}"));
    assert!(
        output.status.success(),
        "build of {name} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let ir = std::fs::read_to_string(dir.join(format!("katana_{atom}.ll")))
        .unwrap_or_else(|err| panic!("no emitted IR for atom {atom}: {err}"));
    std::fs::remove_dir_all(&dir).expect("remove fixture dir");
    ir
}

// The parser lowers `true`/`false` to `Variable("true"|"false")` (see
// mumei-core/src/parser/expr.rs) — codegen used to report them as
// "Undefined variable". They must emit the i64 1/0 constants, matching the
// `zext i1` convention comparisons already produce.
#[test]
fn bool_literals_compile_to_i64_constants() {
    let ir = emit_atom_ir(
        "bool_lit",
        r#"
atom gate(flag: i64) -> i64
    requires: true;
    ensures: true;
    body: {
        let b = flag > 0;
        if b == true { 1 } else { 0 }
    }

atom always() -> i64
    requires: true;
    ensures: true;
    body: {
        if true { 42 } else { 0 }
    }
"#,
        "gate",
    );
    assert!(
        ir.contains("icmp eq i64 %bool_tmp, 1") || ir.contains("icmp eq i64 %b"),
        "b == true must compare against i64 1:\n{ir}"
    );
}

#[test]
fn true_literal_as_if_condition() {
    let ir = emit_atom_ir(
        "bool_always",
        r#"
atom always() -> i64
    requires: true;
    ensures: true;
    body: {
        if true { 42 } else { 0 }
    }
"#,
        "always",
    );
    assert!(
        ir.contains("br i1 true") && ir.contains("[ 42, %then ]"),
        "if true must take the then arm returning 42:\n{ir}"
    );
}

// `&&` / `||` were unrepresentable in codegen ("Unsupported int operator").
// They lower to real short-circuit control flow: the RHS compiles into a
// dedicated `logic.rhs` block guarded by the LHS truth value, so
// `i >= 0 && arr[i] > 0` never runs the array bounds check when i < 0.
#[test]
fn logical_and_short_circuits_rhs() {
    let ir = emit_atom_ir(
        "logic_and",
        r#"
atom sc(arr: [i64], i: i64) -> i64
    requires: i >= 0 && i < len(arr);
    ensures: true;
    body: {
        if i >= 0 && arr[i] > 0 { 1 } else { 0 }
    }

atom either(a: i64, b: i64) -> i64
    requires: true;
    ensures: true;
    body: {
        if a > 0 || b > 0 { 1 } else { 0 }
    }
"#,
        "sc",
    );
    assert!(
        ir.contains("br i1 %logic_lhs, label %logic.rhs, label %logic.merge"),
        "and must branch on the LHS before evaluating the RHS:\n{ir}"
    );
    // The bounds check must live inside logic.rhs — never reached on the
    // short-circuit path.
    let rhs_pos = ir.find("logic.rhs:").unwrap();
    let merge_pos = ir.find("logic.merge:").unwrap();
    let bounds_pos = ir.find("%bounds_check").unwrap();
    assert!(
        rhs_pos < bounds_pos && bounds_pos < merge_pos,
        "bounds check must be inside the RHS block:\n{ir}"
    );
    assert!(
        ir.contains("phi i1 [ false, %entry ]"),
        "short-circuit and yields false without the RHS:\n{ir}"
    );
}
