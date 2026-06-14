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
fn repl_compiles_incremental_atoms_before_eval() {
    let input = r#"
atom a() -> i64
  requires: true;
  ensures: true;
  body: { 1 }
atom b() -> i64
  requires: true;
  ensures: true;
  body: { a() + 1 }
:verify a
:verify b
:eval b()
:quit
"#;
    let (success, stdout, stderr) = run_repl_session(input);

    assert!(
        success,
        "repl should exit successfully\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("Verified: a"));
    assert!(stdout.contains("Verified: b"));
    assert!(stdout.contains("= 2"));
    assert!(
        !stderr.contains("JIT compile warning")
            && !stderr.contains("JIT compile error")
            && !stderr.contains("Execution error"),
        "incremental atoms should resolve in ORC JIT\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
