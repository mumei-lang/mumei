use serde_json::Value;
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

fn write_fake_mumei_agent(dir: &Path) {
    let script = dir.join("mumei-agent");
    fs::write(
        &script,
        r#"#!/bin/sh
case "$1" in
  validate-spec)
    printf '%s\n' '{"success":false,"spec_health_issues":[{"kind":"contradiction","severity":"error","source_line":1,"message":"contradictory natural-language spec"}],"verification_violations":[],"cross_validation_gaps":[],"next_steps":[{"command":"mumei-agent validate-spec --input <spec> --format human"}]}'
    exit 1
    ;;
  validate-code)
    printf '%s\n' '{"success":false,"spec_health_issues":[],"verification_violations":[{"kind":"contract_violation","severity":"error","source_line":3,"message":"return value violates inferred contract"}],"cross_validation_gaps":[],"next_steps":[{"command":"mumei-agent validate-code --input <path> --language python"}]}'
    exit 1
    ;;
esac
exit 0
"#,
    )
    .expect("write fake mumei-agent");
    let mut permissions = fs::metadata(&script)
        .expect("fake mumei-agent metadata")
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    fs::set_permissions(&script, permissions).expect("chmod fake mumei-agent");
}

fn lsp_frame(value: Value) -> Vec<u8> {
    let body = serde_json::to_string(&value).expect("serialize lsp message");
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

fn run_lsp_did_open(path_env: &Path, uri: &str, text: &str) -> (bool, Vec<Value>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("lsp")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("PATH", path_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mumei lsp");

    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "mumei",
                "version": 1,
                "text": text
            }
        }
    });
    child
        .stdin
        .as_mut()
        .expect("open lsp stdin")
        .write_all(&lsp_frame(did_open))
        .expect("write didOpen");
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait for lsp");
    let messages = parse_lsp_messages(&output.stdout);
    (
        output.status.success(),
        messages,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn parse_lsp_messages(output: &[u8]) -> Vec<Value> {
    let mut messages = Vec::new();
    let mut cursor = 0;
    while cursor < output.len() {
        let Some(header_end) = find_bytes(&output[cursor..], b"\r\n\r\n") else {
            break;
        };
        let header_end = cursor + header_end;
        let header = String::from_utf8_lossy(&output[cursor..header_end]);
        let content_length = header
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .and_then(|length| length.parse::<usize>().ok())
            .expect("lsp content length");
        let body_start = header_end + 4;
        let body_end = body_start + content_length;
        messages.push(serde_json::from_slice(&output[body_start..body_end]).expect("lsp json"));
        cursor = body_end;
    }
    messages
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn diagnostics(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .filter(|message| {
            message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
        })
        .flat_map(|message| {
            message
                .pointer("/params/diagnostics")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn lsp_reports_spec_health_issues_for_spec_comments() {
    let fixture_dir = unique_temp_dir("mumei-lsp-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("spec.mm");
    let source = r#"
/// spec: contradiction: balance is both non-negative and negative
atom ok()
    requires: true;
    ensures: result == 1;
    body: { 1 }
"#;
    fs::write(&source_path, source).expect("write mumei source");
    let uri = format!("file://{}", source_path.display());

    let (success, messages, stderr) = run_lsp_did_open(&fixture_dir, &uri, source);
    let diagnostics = diagnostics(&messages);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:\n{stderr}");
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.get("source").and_then(Value::as_str) == Some("mumei-agent"))
        .unwrap_or_else(|| {
            panic!(
                "mumei-agent diagnostic\nmessages:\n{messages:#?}\ndiagnostics:\n{diagnostics:#?}\nstderr:\n{stderr}"
            )
        });
    assert_eq!(
        diagnostic
            .pointer("/range/start/line")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(diagnostic.get("severity").and_then(Value::as_u64), Some(1));
    let message = diagnostic
        .get("message")
        .and_then(Value::as_str)
        .expect("diagnostic message");
    assert!(message.contains("spec_health_issues"), "{message}");
    assert!(
        message.contains("contradictory natural-language spec"),
        "{message}"
    );
    assert!(message.contains("next_steps"), "{message}");
}

#[cfg(unix)]
#[test]
fn lsp_reports_code_verification_violations_for_other_languages() {
    let fixture_dir = unique_temp_dir("mumei-lsp-code-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("service.py");
    let source =
        "def debit(balance, amount):\n    assert balance >= amount\n    return balance + amount\n";
    fs::write(&source_path, source).expect("write python source");
    let uri = format!("file://{}", source_path.display());

    let (success, messages, stderr) = run_lsp_did_open(&fixture_dir, &uri, source);
    let diagnostics = diagnostics(&messages);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:\n{stderr}");
    let diagnostic = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.get("source").and_then(Value::as_str) == Some("mumei-agent"))
        .unwrap_or_else(|| {
            panic!(
                "mumei-agent diagnostic\nmessages:\n{messages:#?}\ndiagnostics:\n{diagnostics:#?}\nstderr:\n{stderr}"
            )
        });
    assert_eq!(
        diagnostic
            .pointer("/range/start/line")
            .and_then(Value::as_u64),
        Some(2)
    );
    assert_eq!(
        diagnostic
            .pointer("/range/start/character")
            .and_then(Value::as_u64),
        Some(0)
    );
    assert_eq!(diagnostic.get("severity").and_then(Value::as_u64), Some(1));
    let message = diagnostic
        .get("message")
        .and_then(Value::as_str)
        .expect("diagnostic message");
    assert!(message.contains("verification_violations"), "{message}");
    assert!(
        message.contains("return value violates inferred contract"),
        "{message}"
    );
}

#[test]
fn lsp_missing_agent_keeps_existing_z3_diagnostics() {
    let fixture_dir = unique_temp_dir("mumei-lsp-no-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create empty PATH dir");

    let source_path = fixture_dir.join("bad.mm");
    let source = r#"
/// spec: balance remains non-negative
atom bad_postcondition(x: i64)
    requires: true;
    ensures: result > 0;
    body: { 0 }
"#;
    fs::write(&source_path, source).expect("write mumei source");
    let uri = format!("file://{}", source_path.display());

    let (success, messages, stderr) = run_lsp_did_open(&fixture_dir, &uri, source);
    let diagnostics = diagnostics(&messages);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:\n{stderr}");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.get("source").and_then(Value::as_str) == Some("mumei-z3")),
        "expected existing Z3 diagnostic\nmessages:\n{messages:#?}\nstderr:\n{stderr}"
    );
    assert!(
        diagnostics.iter().all(
            |diagnostic| diagnostic.get("source").and_then(Value::as_str) != Some("mumei-agent")
        ),
        "missing agent should not emit mumei-agent diagnostics\nmessages:\n{messages:#?}"
    );
}
