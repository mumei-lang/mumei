use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
}

fn run_repl_session(input: &str) -> (bool, String, String) {
    run_repl_session_with_path(input, None)
}

fn run_repl_session_with_path(input: &str, path: Option<&Path>) -> (bool, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .arg("repl")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path {
        command.env("PATH", path);
    }

    let mut child = command.spawn().expect("spawn mumei repl");

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
    assert!(stdout.contains(":verify-spec <path|inline>"));
    assert!(stdout.contains(":verify-code <path>"));
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
    let fixture_dir = unique_temp_dir("mumei-repl-load-dir");
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

#[cfg(unix)]
fn write_fake_mumei_agent(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let script = dir.join("mumei-agent");
    fs::write(
        &script,
        r#"#!/bin/sh
input=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--input" ] || [ "$1" = "--file" ]; then
    shift
    input="$1"
  fi
  shift
done

bad=0
case "$input" in
  *bad-spec*) bad=1 ;;
esac
if [ "$input" ]; then
  while IFS= read -r line; do
    case "$line" in
      *contradiction*|*Contradiction*|*矛盾*) bad=1 ;;
    esac
  done < "$input"
fi

if [ "$bad" = "1" ]; then
  echo '{
  "success": false,
  "spec_health_issues": [
    {
      "kind": "contradiction",
      "message": "contradiction detected"
    }
  ],
  "verification_violations": [],
  "cross_validation_gaps": [],
  "next_steps": [
    {
      "command": "mumei-agent validate-spec --input <spec> --format human"
    }
  ]
}'
  exit 1
fi

echo '{
  "success": true,
  "spec_health_issues": [],
  "verification_violations": [],
  "cross_validation_gaps": [],
  "next_steps": []
}'
"#,
    )
    .expect("write fake mumei-agent");
    let mut permissions = fs::metadata(&script)
        .expect("fake mumei-agent metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script, permissions).expect("chmod fake mumei-agent");
}

#[cfg(unix)]
#[test]
fn repl_verify_spec_reports_agent_health_buckets() {
    let fixture_dir = unique_temp_dir("mumei-repl-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let bad_spec = fixture_dir.join("bad-spec.txt");
    let good_spec = fixture_dir.join("good-spec.txt");
    fs::write(
        &bad_spec,
        "contradiction: balance is both positive and negative",
    )
    .expect("write bad spec");
    fs::write(&good_spec, "balance is non-negative after deposit").expect("write good spec");

    let input = format!(
        ":verify-spec {}\n:verify-spec {}\n:quit\n",
        bad_spec.display(),
        good_spec.display()
    );
    let (success, stdout, stderr) = run_repl_session_with_path(&input, Some(&fixture_dir));
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(
        success,
        "repl should exit successfully\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("FAIL"),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("PASS"),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("spec_health_issues"),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("contradiction detected"),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("verification_violations"),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("cross_validation_gaps"),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("next_steps"),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("Goodbye"),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

#[test]
fn repl_verify_spec_missing_agent_degrades_gracefully() {
    let fixture_dir = unique_temp_dir("mumei-repl-no-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create empty path dir");

    let (success, stdout, stderr) = run_repl_session_with_path(
        ":verify-spec balance is always non-negative\n:quit\n",
        Some(&fixture_dir),
    );
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(
        success,
        "repl should exit successfully\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stderr.contains("mumei-agent not found in PATH"));
    assert!(stdout.contains("Goodbye"));
}
