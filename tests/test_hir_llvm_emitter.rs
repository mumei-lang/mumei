//! Step-5 HIR emitter migration regression tests.
//!
//! The LLVM emitter reads atom metadata (name, params, return type) from
//! `hir_atom.signature` instead of the AST. `HirParam.name` is prefix-free,
//! which is what makes `atom f(consume n: i64)` bind `n` (not `"consume n"`)
//! in the codegen variables map — previously this failed with
//! `Codegen Error: Undefined variable: n`.
//!
//! `binary` output additionally requires a self-recursive `main` to call
//! `__mumei_user_main` (the renamed function), not the C wrapper `main`.

use std::process::Command;

fn write_fixture(name: &str, source: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mumei_hir_llvm_{}_{}", name, std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale fixture dir");
    }
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    let path = dir.join("main.mm");
    std::fs::write(&path, source).expect("write fixture");
    path
}

/// `consume` / `ref` / `ref mut` / untyped params all emit valid LLVM IR,
/// link into a binary, and run.
#[test]
fn build_emits_llvm_ir_for_prefixed_and_untyped_params() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "prefixed_params",
        r#"
atom consume_int(consume n: i64) -> i64
requires: true;
ensures: true;
body: { n };

atom read_ref(ref x: i64) -> i64
requires: true;
ensures: true;
body: { x };

atom bump(ref mut x: i64) -> i64
requires: true;
ensures: true;
body: {
    x = x + 1;
    x
};

atom untyped(x) -> i64
requires: true;
ensures: true;
body: { x };

atom main() -> i64
requires: true;
ensures: true;
body: {
    consume_int(10) + read_ref(5) + bump(7) + untyped(2)
};
"#,
    );
    let out_dir = fixture.parent().unwrap().join("out");

    let output = Command::new(bin)
        .arg("build")
        .arg(&fixture)
        .arg("--emit")
        .arg("llvm-ir")
        .arg("-o")
        .arg(&out_dir)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei build --emit llvm-ir: {err}"));

    assert!(
        output.status.success(),
        "mumei build --emit llvm-ir should succeed for consume/ref/ref mut/untyped params\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // `build -o DIR/STEM --emit llvm-ir` writes `DIR/STEM_<atom>.ll`.
    // The consume param binds under `n` (no `Undefined variable` failure,
    // and the body returns the argument).
    let consume_ll = fixture.parent().unwrap().join("out_consume_int.ll");
    let consume_ir = std::fs::read_to_string(&consume_ll)
        .unwrap_or_else(|err| panic!("read {}: {err}", consume_ll.display()));
    assert!(
        consume_ir.contains("define i64 @consume_int(i64") && consume_ir.contains("ret i64"),
        "consume_int should emit an i64 identity function:\n{consume_ir}"
    );

    // The binary target builds, links, and runs the same atoms.
    let bin_path = fixture.parent().unwrap().join("consume_bin");
    let output = Command::new(bin)
        .arg("build")
        .arg(&fixture)
        .arg("--emit")
        .arg("binary")
        .arg("-o")
        .arg(&bin_path)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei build --emit binary: {err}"));
    assert!(
        output.status.success(),
        "mumei build --emit binary should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(bin_path.exists(), "binary output should exist");

    let run = Command::new(&bin_path)
        .output()
        .unwrap_or_else(|err| panic!("failed to run built binary: {err}"));
    // 10 + 5 + 8 + 2 = 25
    assert_eq!(
        run.status.code(),
        Some(25),
        "binary should return consume_int(10)+read_ref(5)+bump(7)+untyped(2)=25\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove fixture dir");
}

/// Atoms without an explicit `-> ret` annotation get their LLVM return type
/// from `signature.inferred_return_type` (i64 conservative fallback when it
/// cannot be inferred).
#[test]
fn build_emits_ir_return_type_goldens_for_inferred_atoms() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "inferred_ret",
        r#"
atom infer_i64()
requires: true;
ensures: true;
body: { 40 + 2 };

atom infer_f64()
requires: true;
ensures: true;
body: { 1.5 + 2.5 };

atom main() -> i64
requires: true;
ensures: true;
body: { infer_i64() };
"#,
    );
    let out_dir = fixture.parent().unwrap().join("out");

    let output = Command::new(bin)
        .arg("build")
        .arg(&fixture)
        .arg("--emit")
        .arg("llvm-ir")
        .arg("-o")
        .arg(&out_dir)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei build --emit llvm-ir: {err}"));

    assert!(
        output.status.success(),
        "mumei build --emit llvm-ir should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let i64_ll = fixture.parent().unwrap().join("out_infer_i64.ll");
    let f64_ll = fixture.parent().unwrap().join("out_infer_f64.ll");

    let i64_ir = std::fs::read_to_string(&i64_ll)
        .unwrap_or_else(|err| panic!("read {}: {err}", i64_ll.display()));
    assert!(
        i64_ir.contains("define i64 @infer_i64()"),
        "int body should infer an i64 return type:\n{i64_ir}"
    );

    let f64_ir = std::fs::read_to_string(&f64_ll)
        .unwrap_or_else(|err| panic!("read {}: {err}", f64_ll.display()));
    assert!(
        f64_ir.contains("define double @infer_f64()"),
        "float body should infer an f64 return type:\n{f64_ir}"
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove fixture dir");
}

/// A self-recursive `main` builds to a binary that calls
/// `__mumei_user_main`, not the C wrapper `main`.
#[test]
fn build_binary_supports_self_recursive_main() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "self_rec_main",
        r#"
atom main() -> i64
requires: true;
ensures: true;
body: {
    if false { main() } else { 7 }
};
"#,
    );
    let bin_path = fixture.parent().unwrap().join("self_rec_bin");

    let output = Command::new(bin)
        .arg("build")
        .arg(&fixture)
        .arg("--emit")
        .arg("binary")
        .arg("-o")
        .arg(&bin_path)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei build --emit binary: {err}"));

    assert!(
        output.status.success(),
        "self-recursive main should build a binary\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(bin_path.exists(), "binary output should exist");

    let run = Command::new(&bin_path)
        .output()
        .unwrap_or_else(|err| panic!("failed to run built binary: {err}"));
    assert_eq!(
        run.status.code(),
        Some(7),
        "binary should return 7\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove fixture dir");
}
