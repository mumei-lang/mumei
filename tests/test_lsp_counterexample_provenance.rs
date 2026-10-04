use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "mumei-lsp-counterexample-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    dir
}

#[test]
fn verification_diagnostic_includes_counterexample_provenance() {
    // Before this change, the call-site value was "(- 1)" and had no provenance.
    let dir = unique_temp_dir();
    let source_path = dir.join("counterexample.mm");
    let source = "atom positive(x: i64) -> i64\n\
        requires: x > 0;\n\
        ensures: true;\n\
        body: x;\n\
        \n\
        atom caller() -> i64\n\
        requires: true;\n\
        ensures: true;\n\
        body: positive(0 - 1);\n";
    std::fs::write(&source_path, source).expect("write source");
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source_path.display()),
                "languageId": "mumei",
                "version": 1,
                "text": source
            }
        }
    });
    let body = serde_json::to_string(&body).expect("serialize didOpen");
    let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    let mut child = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("lsp")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn LSP");
    child
        .stdin
        .as_mut()
        .expect("open LSP stdin")
        .write_all(frame.as_bytes())
        .expect("write didOpen");
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait for LSP");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let diagnostic = stdout
        .split("Content-Length: ")
        .skip(1)
        .filter_map(|chunk| chunk.split_once("\r\n\r\n").map(|(_, body)| body))
        .filter_map(|body| {
            serde_json::Deserializer::from_str(body)
                .into_iter::<Value>()
                .next()
        })
        .filter_map(Result::ok)
        .find_map(|message| {
            (message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics"))
            .then_some(message)
        })
        .and_then(|message| {
            message
                .pointer("/params/diagnostics")
                .and_then(Value::as_array)
                .and_then(|diagnostics| {
                    diagnostics
                        .iter()
                        .find(|diagnostic| diagnostic.pointer("/data/counterexample/x").is_some())
                })
                .cloned()
        })
        .expect("verification diagnostic");

    assert_eq!(
        diagnostic
            .pointer("/data/counterexample/x")
            .and_then(Value::as_str),
        Some("-1")
    );
    assert_ne!(
        diagnostic
            .pointer("/data/counterexample/x")
            .and_then(Value::as_str),
        Some("(- 1)")
    );
    assert_eq!(
        diagnostic
            .pointer("/data/counterexample_provenance/values/x/status")
            .and_then(Value::as_str),
        Some("raised")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
