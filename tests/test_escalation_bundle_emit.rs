use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

fn temp_dir(name: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("mumei_{name}_{}_{}", std::process::id(), nonce))
}

fn write_unknown_fixture(dir: &std::path::Path) -> std::path::PathBuf {
    let fixture = dir.join("foo.mm");
    std::fs::write(
        &fixture,
        r#"
atom fermat3(x: i64, y: i64, z: i64) -> i64
requires: x > 0 && y > 0 && z > 0;
ensures: x * x * x + y * y * y != z * z * z;
body: { 0 };
"#,
    )
    .expect("write unknown fixture");
    fixture
}

#[test]
fn verify_emit_escalation_bundle_writes_unknown_candidates() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let dir = temp_dir("escalation_bundle_emit");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let fixture = write_unknown_fixture(&dir);
    let bundle_path = fixture.with_extension("escalation-bundle.json");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--solver-timeout")
        .arg("50")
        .arg("--emit")
        .arg("escalation-bundle")
        .arg(&fixture)
        .current_dir(&dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify: {err}"));

    assert!(
        output.status.success(),
        "verify should emit an escalation bundle\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        bundle_path.exists(),
        "expected default bundle path {}",
        bundle_path.display()
    );

    let payload: Value =
        serde_json::from_str(&std::fs::read_to_string(&bundle_path).expect("read bundle json"))
            .expect("parse bundle json");
    let candidates = payload["candidates"].as_array().expect("candidates array");
    assert!(
        candidates.iter().any(|candidate| {
            candidate["z3_check_result"] == "unknown"
                && candidate["z3_result_class"] == "unknown"
                && candidate
                    .get("escalation_reason")
                    .and_then(Value::as_str)
                    .is_some_and(|reason| !reason.is_empty())
        }),
        "expected at least one unknown Lean escalation candidate: {payload:#}"
    );
    assert_eq!(
        payload["summary"]["by_z3_result_class"]["unknown"], 1,
        "summary should include unknown result class"
    );

    std::fs::remove_dir_all(dir).expect("remove temp dir");
}

#[test]
fn verify_escalate_lean_invokes_bridge_and_updates_proof_cert() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let dir = temp_dir("escalate_lean_bridge");
    let bridge_repo = dir.join("mumei-lean");
    let bridge_scripts = bridge_repo.join("scripts");
    std::fs::create_dir_all(&bridge_scripts).expect("create fake bridge dir");
    let fixture = write_unknown_fixture(&dir);
    let cert_path = dir.join("foo.proof.json");
    let bridge_script = bridge_scripts.join("bridge.py");
    std::fs::write(
        &bridge_script,
        r#"
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--escalation-bundle", required=True)
parser.add_argument("--lean-cert-out", required=True)
parser.add_argument("--out-dir")
args = parser.parse_args()

bundle_path = Path(args.escalation_bundle)
payload = json.loads(bundle_path.read_text())
(Path(__file__).resolve().parents[1] / "bridge_called.txt").write_text(str(bundle_path))
for candidate in payload.get("candidates", []):
    candidate["z3_check_result"] = "lean_verified"
    candidate["status"] = "verified"
    candidate["lean_metadata"] = {
        "status": "lean_verified",
        "theorem_name": f"{candidate['name']}_correct",
        "translator_version": candidate.get("translator_version", ""),
        "bridge_lemma_hash": candidate.get("bridge_lemma_hash", ""),
        "proof_path": "generated/Generated/Foo.lean",
        "diagnostics": [],
    }
payload["lean_cert_schema_version"] = "1.0-lean"
Path(args.lean_cert_out).parent.mkdir(parents=True, exist_ok=True)
Path(args.lean_cert_out).write_text(json.dumps(payload))
"#,
    )
    .expect("write fake bridge");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--solver-timeout")
        .arg("50")
        .arg("--escalate-lean")
        .arg("--proof-cert")
        .arg("--output")
        .arg(&cert_path)
        .arg(&fixture)
        .env("MUMEI_LEAN_PATH", &bridge_repo)
        .current_dir(&dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify --escalate-lean: {err}"));

    assert!(
        output.status.success(),
        "--escalate-lean should accept fake bridge output\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(bridge_repo.join("bridge_called.txt").exists());

    let cert: Value =
        serde_json::from_str(&std::fs::read_to_string(&cert_path).expect("read proof cert"))
            .expect("parse proof cert");
    let atom = cert["atoms"].as_array().unwrap().first().unwrap();
    assert_eq!(atom["z3_check_result"], "lean_verified");
    assert_eq!(atom["status"], "verified");
    assert_eq!(atom["lean_metadata"]["status"], "lean_verified");
    assert_eq!(cert["all_verified"], true);

    std::fs::remove_dir_all(dir).expect("remove temp dir");
}

#[test]
fn verify_escalate_lean_rejects_stale_bridge_metadata() {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let dir = temp_dir("escalate_lean_stale_bridge");
    let bridge_repo = dir.join("mumei-lean");
    let bridge_scripts = bridge_repo.join("scripts");
    std::fs::create_dir_all(&bridge_scripts).expect("create fake bridge dir");
    let fixture = write_unknown_fixture(&dir);
    let cert_path = dir.join("foo.proof.json");
    let bridge_script = bridge_scripts.join("bridge.py");
    std::fs::write(
        &bridge_script,
        r#"
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--escalation-bundle", required=True)
parser.add_argument("--lean-cert-out", required=True)
parser.add_argument("--out-dir")
args = parser.parse_args()

payload = json.loads(Path(args.escalation_bundle).read_text())
for candidate in payload.get("candidates", []):
    candidate["z3_check_result"] = "lean_verified"
    candidate["status"] = "verified"
    candidate["translator_version"] = "stale-translator"
    candidate["lean_metadata"] = {
        "status": "lean_verified",
        "theorem_name": "",
        "translator_version": "stale-translator",
        "bridge_lemma_hash": candidate.get("bridge_lemma_hash", ""),
        "proof_path": "generated/Generated/Foo.lean",
        "diagnostics": [],
    }
Path(args.lean_cert_out).parent.mkdir(parents=True, exist_ok=True)
Path(args.lean_cert_out).write_text(json.dumps(payload))
"#,
    )
    .expect("write fake bridge");

    let output = Command::new(bin)
        .arg("verify")
        .arg("--solver-timeout")
        .arg("50")
        .arg("--escalate-lean")
        .arg("--proof-cert")
        .arg("--output")
        .arg(&cert_path)
        .arg(&fixture)
        .env("MUMEI_LEAN_PATH", &bridge_repo)
        .current_dir(&dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify --escalate-lean: {err}"));

    assert!(
        output.status.success(),
        "stale Lean metadata should not crash verification\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let cert: Value =
        serde_json::from_str(&std::fs::read_to_string(&cert_path).expect("read proof cert"))
            .expect("parse proof cert");
    let atom = cert["atoms"].as_array().unwrap().first().unwrap();
    assert_eq!(atom["z3_check_result"], "unknown");
    assert_eq!(cert["all_verified"], false);

    std::fs::remove_dir_all(dir).expect("remove temp dir");
}
