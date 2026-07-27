use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create fixture dir");
    dir
}

fn did_open_diagnostics(source_path: &Path, source: &str) -> Vec<Value> {
    let uri = format!("file://{}", source_path.display());
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "mumei",
                "version": 1,
                "text": source
            }
        }
    });
    let body = serde_json::to_string(&did_open).expect("serialize didOpen");
    let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

    let mut child = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("lsp")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mumei lsp");
    child
        .stdin
        .as_mut()
        .expect("open lsp stdin")
        .write_all(frame.as_bytes())
        .expect("write didOpen");
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait for lsp");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut diagnostics = Vec::new();
    for chunk in stdout.split("Content-Length: ").skip(1) {
        let Some((_, body)) = chunk.split_once("\r\n\r\n") else {
            continue;
        };
        let mut stream = serde_json::Deserializer::from_str(body).into_iter::<Value>();
        let Some(Ok(message)) = stream.next() else {
            continue;
        };
        if message.get("method").and_then(Value::as_str) != Some("textDocument/publishDiagnostics")
        {
            continue;
        }
        if let Some(items) = message
            .pointer("/params/diagnostics")
            .and_then(Value::as_array)
        {
            diagnostics.extend(items.clone());
        }
    }
    diagnostics
}

fn lean_escalation(diagnostic: &Value) -> Option<&Value> {
    diagnostic.pointer("/data/lean_escalation")
}

#[test]
fn lsp_reports_pending_lean_escalation_for_undecided_atoms() {
    let dir = unique_temp_dir("mumei-lsp-lean-pending");
    let source = "atom symbolic_pow(x: i64, y: i64) -> i64\n  requires: x >= 0;\n  ensures: result == x**y && result == x;\n  body: x;\n";
    let source_path = dir.join("pending.mm");
    std::fs::write(&source_path, source).expect("write source");

    let diagnostics = did_open_diagnostics(&source_path, source);
    let _ = std::fs::remove_dir_all(&dir);

    let escalation = diagnostics
        .iter()
        .find_map(lean_escalation)
        .unwrap_or_else(|| panic!("expected a pending escalation diagnostic: {diagnostics:#?}"));
    assert_eq!(
        escalation.get("status").and_then(Value::as_str),
        Some("pending"),
        "{escalation}"
    );
    assert_eq!(
        escalation.get("atom").and_then(Value::as_str),
        Some("symbolic_pow"),
        "{escalation}"
    );
    assert!(
        escalation
            .get("escalation_reason")
            .and_then(Value::as_str)
            .is_some(),
        "expected an escalation reason: {escalation}"
    );
}

#[test]
fn lean_verified_method_does_not_match_a_top_level_atom_with_the_same_short_name() {
    let dir = unique_temp_dir("mumei-lsp-lean-qualified");
    let source = concat!(
        "struct Gauge { value: i64 }\n",
        "\n",
        "atom read(x: i64) -> i64\n",
        "  requires: x >= 0;\n",
        "  ensures: result >= 0;\n",
        "  body: x;\n",
        "\n",
        "impl Gauge {\n",
        "    atom read(self: Gauge) -> i64\n",
        "        requires: self.value >= 0;\n",
        "        ensures: result >= 0;\n",
        "        body: self.value;\n",
        "}\n",
    );
    let source_path = dir.join("qualified.mm");
    let cert_path = dir.join("qualified.proof.json");
    std::fs::write(&source_path, source).expect("write source");

    let generated = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--proof-cert")
        .arg("--output")
        .arg(&cert_path)
        .arg(&source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify --proof-cert");
    assert!(
        generated.status.success(),
        "certificate generation failed:\n{}",
        String::from_utf8_lossy(&generated.stderr)
    );

    let raw = std::fs::read_to_string(&cert_path).expect("read certificate");
    let mut cert: Value = serde_json::from_str(&raw).expect("parse certificate");
    let atoms = cert["atoms"].as_array_mut().expect("certificate atoms");
    let method = atoms
        .iter_mut()
        .find(|atom| atom.get("name").and_then(Value::as_str) == Some("Gauge::read"))
        .expect("qualified method entry in the certificate");
    method["z3_check_result"] = Value::String("lean_verified".to_string());
    method["z3_result_class"] = Value::String("unknown".to_string());
    std::fs::write(&cert_path, cert.to_string()).expect("write patched certificate");

    let diagnostics = did_open_diagnostics(&source_path, source);
    let _ = std::fs::remove_dir_all(&dir);

    let lean: Vec<&Value> = diagnostics
        .iter()
        .filter(|d| d.get("source").and_then(Value::as_str) == Some("mumei-lean"))
        .collect();
    assert_eq!(lean.len(), 1, "{diagnostics:#?}");
    // The method lives on line 8 (0-based); the same-named top-level atom is on line 2.
    assert_eq!(
        lean[0].pointer("/range/start/line").and_then(Value::as_u64),
        Some(8),
        "{:#?}",
        lean[0]
    );
}

#[test]
fn lsp_reports_lean_verified_atoms_from_a_sibling_certificate() {
    let dir = unique_temp_dir("mumei-lsp-lean-verified");
    let source =
        "atom clamp_low(x: i64) -> i64\n  requires: x >= 0;\n  ensures: result >= 0;\n  body: x;\n";
    let source_path = dir.join("verified.mm");
    let cert_path = dir.join("verified.proof.json");
    std::fs::write(&source_path, source).expect("write source");

    let generated = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--proof-cert")
        .arg("--output")
        .arg(&cert_path)
        .arg(&source_path)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify --proof-cert");
    assert!(
        generated.status.success(),
        "certificate generation failed:\n{}",
        String::from_utf8_lossy(&generated.stderr)
    );

    // Rewrite the atom as discharged by mumei-lean, which is what the bridge
    // records when Z3 returned `unknown` and Lean closed the obligation.
    let raw = std::fs::read_to_string(&cert_path).expect("read certificate");
    let mut cert: Value = serde_json::from_str(&raw).expect("parse certificate");
    cert["atoms"][0]["z3_check_result"] = Value::String("lean_verified".to_string());
    cert["atoms"][0]["z3_result_class"] = Value::String("unknown".to_string());
    std::fs::write(&cert_path, cert.to_string()).expect("write patched certificate");

    let diagnostics = did_open_diagnostics(&source_path, source);
    let _ = std::fs::remove_dir_all(&dir);

    let diagnostic = diagnostics
        .iter()
        .find(|d| d.get("source").and_then(Value::as_str) == Some("mumei-lean"))
        .unwrap_or_else(|| panic!("expected a mumei-lean diagnostic: {diagnostics:#?}"));
    assert_eq!(diagnostic.get("severity").and_then(Value::as_u64), Some(3));
    let escalation = lean_escalation(diagnostic).expect("lean_escalation payload");
    assert_eq!(
        escalation.get("status").and_then(Value::as_str),
        Some("lean_verified"),
        "{escalation}"
    );
    assert_eq!(
        escalation.get("atom").and_then(Value::as_str),
        Some("clamp_low"),
        "{escalation}"
    );
}
