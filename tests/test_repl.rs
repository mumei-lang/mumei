use std::fs;
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
    assert!(stdout.contains(":load <file|dir>"));
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
fn repl_evaluates_float_expression_immediately() {
    let (success, stdout, stderr) = run_repl_session(":type 1.5\n1.5\n:quit\n");

    assert!(
        success,
        "repl should exit successfully\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains(": f64"));
    assert!(
        stdout.contains("= 1.5"),
        "float literal should not be executed as i64\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
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

#[test]
fn repl_loads_mm_files_from_directory() {
    let fixture_dir = std::env::temp_dir().join(format!(
        "mumei-repl-load-dir-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let nested_dir = fixture_dir.join("nested");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&nested_dir).expect("create fixture directories");
    fs::write(
        fixture_dir.join("foo.mm"),
        r#"
atom dir_inc(x: i64) -> i64
  requires: true;
  ensures: result == x + 1;
  body: { x + 1 }
"#,
    )
    .expect("write foo fixture");
    fs::write(
        nested_dir.join("bar.mm"),
        r#"
atom dir_double(x: i64) -> i64
  requires: true;
  ensures: result == x * 2;
  body: { x * 2 }
"#,
    )
    .expect("write bar fixture");

    let input = format!(":load {}\ndir_inc(1)\n:quit\n", fixture_dir.display());
    let (success, stdout, stderr) = run_repl_session(&input);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(
        success,
        "repl should exit successfully\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("Loaded 1 definition(s)"));
    assert!(stdout.contains("Total: 2 definition(s) from 2 file(s)"));
    assert!(stdout.contains("Verified: dir_inc"));
    assert!(stdout.contains("Verified: dir_double"));
    assert!(stdout.contains("= 2"));
}
