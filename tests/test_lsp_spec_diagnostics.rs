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
input=""
case "$1" in
  validate-spec)
    input="$3"
    clean=0
    silent=0
    if [ -n "$input" ]; then
      while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
          *clean*) clean=1 ;;
          *silent*) silent=1 ;;
          *cross*) cross=1 ;;
        esac
      done < "$input"
    fi
    if [ "$clean" = "1" ]; then
      printf '%s\n' '{"success":true,"spec_health_issues":[],"verification_violations":[],"cross_validation_gaps":[],"next_steps":[]}'
      exit 0
    fi
    if [ "$cross" = "1" ]; then
      printf '%s\n' '{"success":false,"spec_health_issues":[],"verification_violations":[],"cross_validation_gaps":[{"kind":"missing_contract","severity":"warning","source_line":1,"message":"spec does not cover division by zero"}],"next_steps":[{"command":"mumei-agent validate-spec --input <spec> --format human"}]}'
      exit 1
    fi
    if [ "$silent" = "1" ]; then
      printf '%s\n' '{"success":false,"spec_health_issues":[],"verification_violations":[],"cross_validation_gaps":[],"next_steps":[{"command":"mumei-agent validate-spec --input <spec> --format human"}]}'
      exit 1
    fi
    printf '%s\n' '{"success":false,"spec_health_issues":[{"kind":"contradiction","severity":"error","source_line":1,"message":"contradictory natural-language spec"}],"verification_violations":[],"cross_validation_gaps":[],"next_steps":[{"command":"mumei-agent validate-spec --input <spec> --format human"}]}'
    exit 1
    ;;
  validate-code)
    input="$3"
    has_unverifiable=0
    has_bug=0
    if [ -n "$input" ] && [ -f "$input" ]; then
      while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
          *unverifiable*|*z3*could*not*decide*) has_unverifiable=1 ;;
          *bug*|*violation*|*error*) has_bug=1 ;;
        esac
      done < "$input"
    fi
    if [ "$has_unverifiable" = "1" ]; then
      printf '%s\n' '{"success":false,"spec_health_issues":[],"verification_violations":[],"verification_status":"unverifiable","cross_validation_gaps":[],"next_steps":[{"command":"mumei-agent validate-code --input <path> --language python"}]}'
      exit 1
    fi
    if [ "$has_bug" = "1" ]; then
      printf '%s\n' '{"success":false,"spec_health_issues":[],"verification_violations":[{"kind":"contract_violation","severity":"error","source_line":3,"message":"return value violates inferred contract"}],"verification_status":"refuted","cross_validation_gaps":[],"next_steps":[{"command":"mumei-agent validate-code --input <path> --language python"}]}'
      exit 1
    fi
    printf '%s\n' '{"success":true,"spec_health_issues":[],"verification_violations":[],"verification_status":"verified","cross_validation_gaps":[],"next_steps":[]}'
    exit 0
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

fn run_lsp_session(path_env: &Path, messages: &[Value]) -> (bool, Vec<Value>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("lsp")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("PATH", path_env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mumei lsp");

    let stdin = child.stdin.as_mut().expect("open lsp stdin");
    for message in messages {
        stdin
            .write_all(&lsp_frame(message.clone()))
            .expect("write lsp message");
    }
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait for lsp");
    let messages = parse_lsp_messages(&output.stdout);
    (
        output.status.success(),
        messages,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn run_lsp_did_open(path_env: &Path, uri: &str, text: &str) -> (bool, Vec<Value>, String) {
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
    run_lsp_session(path_env, &[did_open])
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

fn publish_diagnostics_for_uri(messages: &[Value], uri: &str) -> Vec<Vec<Value>> {
    messages
        .iter()
        .filter(|message| {
            message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
                && message
                    .pointer("/params/uri")
                    .and_then(Value::as_str)
                    .is_some_and(|u| u == uri)
        })
        .map(|message| {
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
fn lsp_suppresses_agent_diagnostics_for_clean_spec_comments() {
    let fixture_dir = unique_temp_dir("mumei-lsp-clean-spec-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("clean_spec.mm");
    let source = r#"
/// spec: clean balance is non-negative
atom ok(x: i64)
    requires: x >= 0;
    ensures: result == x;
    body: { x }
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
            .all(|d| d.get("source").and_then(Value::as_str) != Some("mumei-agent")),
        "clean spec should not emit mumei-agent diagnostics\nmessages:\n{messages:#?}"
    );
}

#[cfg(unix)]
#[test]
fn lsp_reports_spec_validation_failure_when_agent_returns_empty_issues() {
    let fixture_dir = unique_temp_dir("mumei-lsp-silent-spec-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("silent_spec.mm");
    let source = r#"
/// spec: silent failure: balance is non-negative
atom ok(x: i64)
    requires: x >= 0;
    ensures: result == x;
    body: { x }
"#;
    fs::write(&source_path, source).expect("write mumei source");
    let uri = format!("file://{}", source_path.display());

    let (success, messages, stderr) = run_lsp_did_open(&fixture_dir, &uri, source);
    let diagnostics = diagnostics(&messages);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:\n{stderr}");
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.get("source").and_then(Value::as_str) == Some("mumei-agent"))
        .unwrap_or_else(|| {
            panic!(
                "expected mumei-agent fallback diagnostic\nmessages:\n{messages:#?}\ndiagnostics:\n{diagnostics:#?}"
            )
        });
    assert_eq!(diagnostic.get("severity").and_then(Value::as_u64), Some(1));
    let message = diagnostic
        .get("message")
        .and_then(Value::as_str)
        .expect("diagnostic message");
    assert!(message.contains("spec validation failed"), "{message}");
}

#[cfg(unix)]
#[test]
fn lsp_reports_spec_cross_validation_gaps() {
    let fixture_dir = unique_temp_dir("mumei-lsp-cross-gap-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("cross_gap_spec.mm");
    let source = r#"
/// spec: cross gap: balance is non-negative
atom ok(x: i64)
    requires: x >= 0;
    ensures: result == x;
    body: { x }
"#;
    fs::write(&source_path, source).expect("write mumei source");
    let uri = format!("file://{}", source_path.display());

    let (success, messages, stderr) = run_lsp_did_open(&fixture_dir, &uri, source);
    let diagnostics = diagnostics(&messages);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:\n{stderr}");
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.get("source").and_then(Value::as_str) == Some("mumei-agent"))
        .unwrap_or_else(|| {
            panic!(
                "expected mumei-agent diagnostic\nmessages:\n{messages:#?}\ndiagnostics:\n{diagnostics:#?}"
            )
        });
    let message = diagnostic
        .get("message")
        .and_then(Value::as_str)
        .expect("diagnostic message");
    assert!(
        message.contains("cross_validation_gaps"),
        "expected cross_validation_gaps diagnostic, got: {message}"
    );
    assert!(
        message.contains("spec does not cover division by zero"),
        "{message}"
    );
}

#[cfg(unix)]
#[test]
fn lsp_reports_spec_validation_failure_alongside_other_diagnostics() {
    let fixture_dir = unique_temp_dir("mumei-lsp-silent-spec-with-z3-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("silent_spec.mm");
    let source = r#"
/// spec: silent failure: balance is non-negative
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
    let agent_diagnostic = diagnostics
        .iter()
        .find(|d| d.get("source").and_then(Value::as_str) == Some("mumei-agent"))
        .unwrap_or_else(|| {
            panic!(
                "expected mumei-agent fallback diagnostic\nmessages:\n{messages:#?}\ndiagnostics:\n{diagnostics:#?}"
            )
        });
    let message = agent_diagnostic
        .get("message")
        .and_then(Value::as_str)
        .expect("diagnostic message");
    assert!(message.contains("spec validation failed"), "{message}");
}

#[cfg(unix)]
#[test]
fn lsp_reports_code_verification_violations_for_other_languages() {
    let fixture_dir = unique_temp_dir("mumei-lsp-code-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("service.py");
    let source = "def debit(balance, amount):\n    # bug\n    return balance + amount\n";
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
    assert!(
        diagnostics
            .iter()
            .filter(|d| d.get("source").and_then(Value::as_str) == Some("mumei-agent"))
            .any(|d| {
                d.get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|m| m.contains("verification_status: refuted"))
            }),
        "expected verification_status: refuted diagnostic\nmessages:\n{messages:#?}\ndiagnostics:\n{diagnostics:#?}"
    );
}

#[cfg(unix)]
#[test]
fn lsp_reports_code_verification_violations_for_typescript() {
    let fixture_dir = unique_temp_dir("mumei-lsp-ts-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("service.ts");
    let source = "function debit(balance: number, amount: number): number {\n  // bug\n  return balance + amount;\n}\n";
    fs::write(&source_path, source).expect("write typescript source");
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
                "mumei-agent diagnostic for .ts\nmessages:\n{messages:#?}\ndiagnostics:\n{diagnostics:#?}\nstderr:\n{stderr}"
            )
        });
    assert_eq!(
        diagnostic
            .pointer("/range/start/line")
            .and_then(Value::as_u64),
        Some(2)
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
    assert!(
        diagnostics
            .iter()
            .filter(|d| d.get("source").and_then(Value::as_str) == Some("mumei-agent"))
            .any(|d| {
                d.get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|m| m.contains("verification_status: refuted"))
            }),
        "expected verification_status: refuted diagnostic\nmessages:\n{messages:#?}\ndiagnostics:\n{diagnostics:#?}"
    );
}

#[cfg(unix)]
#[test]
fn lsp_reports_code_verification_violations_for_tsx() {
    let fixture_dir = unique_temp_dir("mumei-lsp-tsx-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("component.tsx");
    let source = "const App = () => {\n  // bug\n  return <div>hello</div>;\n}\n";
    fs::write(&source_path, source).expect("write tsx source");
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
                "mumei-agent diagnostic for .tsx\nmessages:\n{messages:#?}\ndiagnostics:\n{diagnostics:#?}\nstderr:\n{stderr}"
            )
        });
    assert_eq!(diagnostic.get("severity").and_then(Value::as_u64), Some(1));
    let message = diagnostic
        .get("message")
        .and_then(Value::as_str)
        .expect("diagnostic message");
    assert!(message.contains("verification_violations"), "{message}");
    assert!(
        diagnostics
            .iter()
            .filter(|d| d.get("source").and_then(Value::as_str) == Some("mumei-agent"))
            .any(|d| {
                d.get("message")
                    .and_then(Value::as_str)
                    .is_some_and(|m| m.contains("verification_status: refuted"))
            }),
        "expected verification_status: refuted diagnostic\nmessages:\n{messages:#?}\ndiagnostics:\n{diagnostics:#?}"
    );
}

#[cfg(unix)]
#[test]
fn lsp_suppresses_agent_diagnostics_for_verified_python() {
    let fixture_dir = unique_temp_dir("mumei-lsp-clean-py-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("good_service.py");
    let source = "def credit(balance, amount):\n    return balance + amount\n";
    fs::write(&source_path, source).expect("write python source");
    let uri = format!("file://{}", source_path.display());

    let (success, messages, stderr) = run_lsp_did_open(&fixture_dir, &uri, source);
    let diagnostics = diagnostics(&messages);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:{stderr}");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.get("source").and_then(Value::as_str) != Some("mumei-agent")),
        "verified code should not emit mumei-agent diagnostics\nmessages:\n{messages:#?}"
    );
}

#[cfg(unix)]
#[test]
fn lsp_suppresses_agent_diagnostics_for_verified_typescript() {
    let fixture_dir = unique_temp_dir("mumei-lsp-clean-ts-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("good_service.ts");
    let source = "function credit(balance: number, amount: number): number {\n  return balance + amount;\n}\n";
    fs::write(&source_path, source).expect("write typescript source");
    let uri = format!("file://{}", source_path.display());

    let (success, messages, stderr) = run_lsp_did_open(&fixture_dir, &uri, source);
    let diagnostics = diagnostics(&messages);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:{stderr}");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.get("source").and_then(Value::as_str) != Some("mumei-agent")),
        "verified code should not emit mumei-agent diagnostics\nmessages:\n{messages:#?}"
    );
}

#[cfg(unix)]
#[test]
fn lsp_suppresses_agent_diagnostics_for_verified_tsx() {
    let fixture_dir = unique_temp_dir("mumei-lsp-clean-tsx-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("good_service.tsx");
    let source = "function Credit(balance: number, amount: number): JSX.Element {\n  return <div>{balance + amount}</div>;\n}\n";
    fs::write(&source_path, source).expect("write tsx source");
    let uri = format!("file://{}", source_path.display());

    let (success, messages, stderr) = run_lsp_did_open(&fixture_dir, &uri, source);
    let diagnostics = diagnostics(&messages);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:{stderr}");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.get("source").and_then(Value::as_str) != Some("mumei-agent")),
        "verified code should not emit mumei-agent diagnostics\nmessages:\n{messages:#?}"
    );
}

#[cfg(unix)]
#[test]
fn lsp_suppresses_agent_diagnostics_for_verified_rust() {
    let fixture_dir = unique_temp_dir("mumei-lsp-clean-rs-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("good_service.rs");
    let source = "fn credit(balance: i64, amount: i64) -> i64 {\n    balance + amount\n}\n";
    fs::write(&source_path, source).expect("write rust source");
    let uri = format!("file://{}", source_path.display());

    let (success, messages, stderr) = run_lsp_did_open(&fixture_dir, &uri, source);
    let diagnostics = diagnostics(&messages);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:{stderr}");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.get("source").and_then(Value::as_str) != Some("mumei-agent")),
        "verified code should not emit mumei-agent diagnostics\nmessages:\n{messages:#?}"
    );
}

#[cfg(unix)]
#[test]
fn lsp_suppresses_agent_diagnostics_for_verified_go() {
    let fixture_dir = unique_temp_dir("mumei-lsp-clean-go-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("good_service.go");
    let source = "package main\n\nfunc credit(balance int64, amount int64) int64 {\n    return balance + amount\n}\n";
    fs::write(&source_path, source).expect("write go source");
    let uri = format!("file://{}", source_path.display());

    let (success, messages, stderr) = run_lsp_did_open(&fixture_dir, &uri, source);
    let diagnostics = diagnostics(&messages);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:{stderr}");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.get("source").and_then(Value::as_str) != Some("mumei-agent")),
        "verified code should not emit mumei-agent diagnostics\nmessages:\n{messages:#?}"
    );
}

#[cfg(unix)]
#[test]
fn lsp_reports_unverifiable_code_diagnostic() {
    let fixture_dir = unique_temp_dir("mumei-lsp-unverifiable-py-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("unverifiable_service.py");
    let source = "def complex(balance, amount):\n    # z3 could not decide\n    return balance\n";
    fs::write(&source_path, source).expect("write python source");
    let uri = format!("file://{}", source_path.display());

    let (success, messages, stderr) = run_lsp_did_open(&fixture_dir, &uri, source);
    let diagnostics = diagnostics(&messages);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:{stderr}");
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.get("source").and_then(Value::as_str) == Some("mumei-agent"))
        .unwrap_or_else(|| {
            panic!(
                "expected mumei-agent diagnostic\nmessages:\n{messages:#?}\ndiagnostics:\n{diagnostics:#?}"
            )
        });
    let message = diagnostic
        .get("message")
        .and_then(Value::as_str)
        .expect("diagnostic message");
    assert!(
        message.contains("verification_status: unverifiable"),
        "{message}"
    );
    assert_eq!(diagnostic.get("severity").and_then(Value::as_u64), Some(2));
    let related = diagnostic
        .get("relatedInformation")
        .and_then(Value::as_array)
        .expect("relatedInformation");
    assert!(
        related.iter().any(|info| {
            info.get("message")
                .and_then(Value::as_str)
                .is_some_and(|m| m.contains("mumei-agent validate-code"))
        }),
        "expected next_steps in relatedInformation: {related:?}"
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

#[cfg(unix)]
#[test]
fn lsp_updates_diagnostics_on_did_change_for_code() {
    let fixture_dir = unique_temp_dir("mumei-lsp-did-change-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("service.py");
    let clean_source = "def credit(balance, amount):\n    return balance + amount\n";
    let bad_source = "def credit(balance, amount):\n    # bug\n    return balance\n";
    fs::write(&source_path, clean_source).expect("write python source");
    let uri = format!("file://{}", source_path.display());

    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": &uri,
                "languageId": "python",
                "version": 1,
                "text": clean_source
            }
        }
    });
    let did_change = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didChange",
        "params": {
            "textDocument": {
                "uri": &uri,
                "version": 2
            },
            "contentChanges": [{"text": bad_source}]
        }
    });

    let (success, messages, stderr) = run_lsp_session(&fixture_dir, &[did_open, did_change]);
    let per_message_diagnostics = publish_diagnostics_for_uri(&messages, &uri);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:\n{stderr}");
    assert!(
        per_message_diagnostics.len() >= 2,
        "expected didOpen + didChange publishDiagnostics, got {} message(s)\nmessages:\n{messages:#?}",
        per_message_diagnostics.len()
    );
    assert!(
        !per_message_diagnostics[0]
            .iter()
            .any(|d| d.get("source").and_then(Value::as_str) == Some("mumei-agent")),
        "clean open should not emit mumei-agent diagnostics\nfirst diagnostics:\n{:#?}",
        per_message_diagnostics[0]
    );
    let changed = per_message_diagnostics
        .last()
        .expect("didChange diagnostics");
    let diagnostic = changed
        .iter()
        .find(|d| d.get("source").and_then(Value::as_str) == Some("mumei-agent"))
        .unwrap_or_else(|| {
            panic!(
                "expected mumei-agent diagnostic after didChange\nmessages:\n{messages:#?}\ndiagnostics:\n{changed:#?}"
            )
        });
    let message = diagnostic
        .get("message")
        .and_then(Value::as_str)
        .expect("diagnostic message");
    assert!(
        message.contains("verification_violations"),
        "expected verification_violations diagnostic, got: {message}"
    );
}

#[cfg(unix)]
#[test]
fn lsp_updates_diagnostics_on_did_change_for_spec() {
    let fixture_dir = unique_temp_dir("mumei-lsp-did-change-spec-agent");
    let _ = fs::remove_dir_all(&fixture_dir);
    fs::create_dir_all(&fixture_dir).expect("create fake agent dir");
    write_fake_mumei_agent(&fixture_dir);

    let source_path = fixture_dir.join("review.mm");
    let clean_source = r#"/// spec: clean
atom ok(x: i64)
    requires: x >= 0;
    ensures: result == x;
    body: { x }
"#;
    let bad_source = r#"/// spec: contradiction
atom ok(x: i64)
    requires: x >= 0;
    ensures: result == x;
    body: { x }
"#;
    fs::write(&source_path, clean_source).expect("write mumei source");
    let uri = format!("file://{}", source_path.display());

    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": &uri,
                "languageId": "mumei",
                "version": 1,
                "text": clean_source
            }
        }
    });
    let did_change = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didChange",
        "params": {
            "textDocument": {
                "uri": &uri,
                "version": 2
            },
            "contentChanges": [{"text": bad_source}]
        }
    });

    let (success, messages, stderr) = run_lsp_session(&fixture_dir, &[did_open, did_change]);
    let per_message_diagnostics = publish_diagnostics_for_uri(&messages, &uri);
    let _ = fs::remove_dir_all(&fixture_dir);

    assert!(success, "lsp should exit successfully\nstderr:\n{stderr}");
    assert!(
        per_message_diagnostics.len() >= 2,
        "expected didOpen + didChange publishDiagnostics, got {} message(s)\nmessages:\n{messages:#?}",
        per_message_diagnostics.len()
    );
    assert!(
        !per_message_diagnostics[0]
            .iter()
            .any(|d| d.get("source").and_then(Value::as_str) == Some("mumei-agent")),
        "clean spec open should not emit mumei-agent diagnostics\nfirst diagnostics:\n{:#?}",
        per_message_diagnostics[0]
    );
    let changed = per_message_diagnostics
        .last()
        .expect("didChange diagnostics");
    let diagnostic = changed
        .iter()
        .find(|d| d.get("source").and_then(Value::as_str) == Some("mumei-agent"))
        .unwrap_or_else(|| {
            panic!(
                "expected mumei-agent diagnostic after didChange\nmessages:\n{messages:#?}\ndiagnostics:\n{changed:#?}"
            )
        });
    let message = diagnostic
        .get("message")
        .and_then(Value::as_str)
        .expect("diagnostic message");
    assert!(
        message.contains("spec_health_issues"),
        "expected spec_health_issues diagnostic, got: {message}"
    );
}
