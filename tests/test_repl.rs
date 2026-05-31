use std::io::Write;
use std::process::{Command, Stdio};

fn run_repl_session(input: &str) -> (bool, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("repl")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mumei repl");

    child
        .stdin
        .as_mut()
        .expect("open repl stdin")
        .write_all(input.as_bytes())
        .expect("write repl input");

    let output = child.wait_with_output().expect("wait for repl");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn repl_launches_and_shows_help() {
    let (success, stdout, stderr) = run_repl_session(":help\n:quit\n");

    assert!(
        success,
        "repl should exit successfully\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("Mumei REPL"));
    assert!(stdout.contains(":type <expr>"));
    assert!(stdout.contains(":verify <atom>"));
    assert!(stdout.contains("Goodbye"));
}

#[test]
fn repl_evaluates_expression_immediately() {
    let (success, stdout, stderr) = run_repl_session("1 + 2\n:quit\n");

    assert!(
        success,
        "repl should exit successfully\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("= 3"));
}

#[test]
fn repl_compiles_atom_definition_and_calls_it() {
    let input = r#"
atom inc(x: i64) -> i64
  requires: true;
  ensures: result == x + 1;
  body: { x + 1 }
inc(5)
:quit
"#;
    let (success, stdout, stderr) = run_repl_session(input);

    assert!(
        success,
        "repl should exit successfully\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("Verified: inc"));
    assert!(stdout.contains("= 6"));
}

#[test]
fn repl_loads_json_ffi_and_executes_symbol() {
    let (success, stdout, stderr) =
        run_repl_session(":load std/json.mm\n:eval json_parse(\"{}\")\n:quit\n");

    assert!(
        success,
        "repl should exit successfully\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("Loaded"));
    assert!(stdout.contains("= "));
    assert!(
        !stderr.contains("Symbols not found") && !stderr.contains("JIT compile error"),
        "json FFI symbols should resolve in JIT\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
