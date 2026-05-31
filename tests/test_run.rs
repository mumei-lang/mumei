use std::path::Path;
use std::process::Command;

fn write_fixture(name: &str, source: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mumei_run_test_{}_{}", name, std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale run fixture dir");
    }
    std::fs::create_dir_all(&dir).expect("create run fixture dir");
    let path = dir.join("main.mm");
    std::fs::write(&path, source).expect("write run fixture");
    path
}

#[test]
fn run_command_verifies_links_and_executes_atom_main() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "basic",
        r#"
atom helper()
requires: true;
ensures: result == 0;
body: { 0 };

atom main()
requires: true;
ensures: result == 0;
body: { helper() };
"#,
    );

    let output = Command::new(bin)
        .arg("run")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei run: {err}"));

    assert!(
        output.status.success(),
        "mumei run should return atom main exit code\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Mumei Run: verify"));
    assert!(stdout.contains("Linking"));

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove run fixture dir");
}

#[test]
fn run_command_can_emit_persistent_binary() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "binary",
        r#"
atom main()
requires: true;
ensures: result == 0;
body: { 0 };
"#,
    );
    let out_path = fixture.parent().unwrap().join("app");

    let output = Command::new(bin)
        .arg("run")
        .arg(&fixture)
        .arg("--emit")
        .arg("binary")
        .arg("-o")
        .arg(&out_path)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei run --emit binary: {err}"));

    assert!(
        output.status.success(),
        "mumei run --emit binary should succeed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(Path::new(&out_path).exists(), "binary output should exist");

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove run fixture dir");
}

#[test]
fn run_command_links_rust_ffi_runtime() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture = write_fixture(
        "rust_ffi",
        r#"
extern "Rust" {
    fn json_from_bool(value: i64) -> i64
        requires: value >= 0 && value <= 1;
        ensures: result >= 0;
}

atom main()
requires: true;
ensures: result >= 0;
body: { json_from_bool(0) };
"#,
    );

    let output = Command::new(bin)
        .arg("run")
        .arg(&fixture)
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei run with Rust FFI: {err}"));

    assert_eq!(
        output.status.code(),
        Some(1),
        "json_from_bool should execute via the linked Rust FFI runtime and return handle 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(fixture.parent().unwrap()).expect("remove run fixture dir");
}
