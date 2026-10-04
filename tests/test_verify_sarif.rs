use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const SARIF_SOURCE: &str = r#"
atom always_false(x: i64) -> i64
requires: true;
ensures "always false": result < x;
body: { x + 1 };

atom some_inputs(x: i64) -> i64
requires: x >= 0;
ensures "large result": result > 10;
body: { x + 1 };

trusted atom conditional(x: i64) -> i64
requires: true;
ensures: (x != 5 || result == 0) && (x != 5 || result == 1);
body: 0;

atom vacuous_caller(x: i64) -> i64
requires: x == 5;
ensures: result >= 0;
body: conditional(x);

atom fermat3(x: i64, y: i64, z: i64) -> i64
requires: x > 0 && y > 0 && z > 0;
ensures: x * x * x + y * y * y != z * z * z;
body: { 0 };

atom pos(y: i64) -> i64
requires: y > 0;
ensures: result == y;
body: y;

atom cover_unreachable(x: i64) -> i64
requires: x < 0;
cover "positive call": pos(x) == 1;
body: x;

atom covered(x: i64) -> i64
requires: x >= 0 && x < 3;
ensures: result == x;
cover "zero witness": result == 0;
body: x;

atom trusted_clauses(x: i64) -> i64
requires assume: x > -10;
ensures assume "trusted result": result > 100;
body: 0;
"#;

fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "mumei_verify_sarif_{name}_{}_{}",
        std::process::id(),
        nonce
    ));
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    directory
}

fn run_verify(input: &Path, cwd: &Path, report_dir: &Path, extra_args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(input)
        .arg("--json")
        .arg("--cache-scope")
        .arg("global")
        .arg("--solver-timeout")
        .arg("50")
        .arg("--disable-spurious-detection")
        .arg("--report-dir")
        .arg(report_dir)
        .args(extra_args)
        .current_dir(cwd)
        .output()
        .expect("run mumei verify")
}

fn sarif_result<'a>(results: &'a [Value], atom: &str, rule_id: &str) -> &'a Value {
    results
        .iter()
        .find(|result| {
            result["properties"]["atom"].as_str() == Some(atom)
                && result["ruleId"].as_str() == Some(rule_id)
        })
        .unwrap_or_else(|| panic!("missing {rule_id} result for {atom}: {results:#?}"))
}

fn read_sarif(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read report.sarif"))
        .expect("parse report.sarif")
}

#[test]
fn sarif_preserves_json_and_exit_code_and_maps_verification_findings() {
    let root = temp_dir("single");
    let fixture = root.join("features.mm");
    std::fs::write(&fixture, SARIF_SOURCE).expect("write fixture");
    let baseline_cwd = root.join("baseline");
    let sarif_cwd = root.join("with_sarif");
    let baseline_reports = baseline_cwd.join("reports");
    let sarif_reports = sarif_cwd.join("reports");
    std::fs::create_dir_all(&baseline_cwd).expect("create baseline cwd");
    std::fs::create_dir_all(&sarif_cwd).expect("create SARIF cwd");

    let baseline = run_verify(&fixture, &baseline_cwd, &baseline_reports, &[]);
    let with_sarif = run_verify(&fixture, &sarif_cwd, &sarif_reports, &["--emit", "sarif"]);
    assert_eq!(baseline.status.code(), with_sarif.status.code());
    assert_eq!(baseline.stdout, with_sarif.stdout);

    let document = read_sarif(&sarif_reports.join("report.sarif"));
    assert_eq!(
        document["$schema"],
        "https://json.schemastore.org/sarif-2.1.0.json"
    );
    assert_eq!(document["version"], "2.1.0");
    let run = &document["runs"][0];
    assert_eq!(run["tool"]["driver"]["name"], "mumei");
    assert_eq!(
        run["invocations"][0]["exitCode"],
        with_sarif.status.code().unwrap()
    );
    let results = run["results"].as_array().expect("SARIF results");
    let rules = run["tool"]["driver"]["rules"]
        .as_array()
        .expect("SARIF rules");
    for result in results {
        let index = result["ruleIndex"].as_u64().expect("ruleIndex") as usize;
        assert_eq!(rules[index]["id"], result["ruleId"]);
    }

    let false_result = sarif_result(results, "always_false", "postcondition_violated");
    assert_eq!(false_result["level"], "error");
    assert_eq!(false_result["properties"]["outcome"], "always_false");
    assert_eq!(false_result["properties"]["label"], "always false");
    assert!(false_result["message"]["text"]
        .as_str()
        .unwrap()
        .contains("\"always false\""));
    assert!(false_result["properties"].get("counterexample").is_some());
    assert!(false_result["properties"]
        .get("counterexample_fidelity")
        .is_some());

    let partial_result = sarif_result(results, "some_inputs", "postcondition_violated");
    assert_eq!(
        partial_result["properties"]["outcome"],
        "fails_on_some_inputs"
    );
    assert!(partial_result["message"]["text"]
        .as_str()
        .unwrap()
        .contains("\"large result\""));
    assert_eq!(
        sarif_result(results, "vacuous_caller", "vacuous")["level"],
        "warning"
    );
    assert_eq!(
        sarif_result(results, "vacuous_caller", "invariant_violated")["level"],
        "error"
    );
    assert_eq!(
        sarif_result(results, "fermat3", "unknown")["level"],
        "warning"
    );
    assert_eq!(
        sarif_result(results, "cover_unreachable", "cover_unreachable")["level"],
        "error"
    );
    let covered = sarif_result(results, "covered", "covered");
    assert_eq!(covered["kind"], "pass");
    assert_eq!(covered["level"], "none");
    assert_eq!(covered["properties"]["witness"]["result"], "0");
    assert_eq!(
        sarif_result(results, "trusted_clauses", "assumed_clause")["level"],
        "note"
    );
    assert!(
        sarif_result(results, "trusted_clauses", "assumed_clause")["message"]["text"]
            .as_str()
            .unwrap()
            .contains("trusted, not proved")
    );
    std::fs::remove_dir_all(root).expect("remove fixture directory");
}

#[test]
fn directory_sarif_aggregates_results_from_every_file() {
    let root = temp_dir("directory");
    let input_dir = root.join("input");
    let cwd = root.join("cwd");
    let reports = cwd.join("reports");
    std::fs::create_dir_all(&input_dir).expect("create input directory");
    std::fs::create_dir_all(&cwd).expect("create current directory");
    std::fs::write(
        input_dir.join("first.mm"),
        "atom first(x: i64) -> i64\nrequires: true;\nensures: result < x;\nbody: { x + 1 };\n",
    )
    .expect("write first fixture");
    std::fs::write(
        input_dir.join("second.mm"),
        "atom second(x: i64) -> i64\nrequires: true;\nensures: result < x;\nbody: { x + 1 };\n",
    )
    .expect("write second fixture");

    let output = run_verify(&input_dir, &cwd, &reports, &["--emit", "sarif"]);
    assert_eq!(output.status.code(), Some(1));
    let document = read_sarif(&reports.join("report.sarif"));
    let results = document["runs"][0]["results"].as_array().unwrap();
    assert!(results
        .iter()
        .any(|result| result["properties"]["atom"] == "first"));
    assert!(results
        .iter()
        .any(|result| result["properties"]["atom"] == "second"));
    assert_eq!(document["runs"][0]["invocations"][0]["exitCode"], 1);
    std::fs::remove_dir_all(root).expect("remove fixture directory");
}

#[test]
fn sarif_includes_lean_axiom_audit_rejections() {
    let root = temp_dir("axiom_rejected");
    let bridge_repo = root.join("mumei-lean");
    let bridge_scripts = bridge_repo.join("scripts");
    let fixture = root.join("unknown.mm");
    let reports = root.join("reports");
    std::fs::create_dir_all(&bridge_scripts).expect("create fake bridge directory");
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
    std::fs::write(
        bridge_scripts.join("bridge.py"),
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
    candidate["lean_metadata"] = {
        "status": "lean_verified",
        "theorem_name": f"{candidate['name']}_correct",
        "translator_version": candidate.get("translator_version", ""),
        "bridge_lemma_hash": candidate.get("bridge_lemma_hash", ""),
        "proof_path": "generated/Generated/Foo.lean",
        "diagnostics": [],
        "axiom_audit": "rejected",
        "kernel_axioms": ["propext", "sorryAx"],
    }
payload["lean_cert_schema_version"] = "1.0-lean"
Path(args.lean_cert_out).parent.mkdir(parents=True, exist_ok=True)
Path(args.lean_cert_out).write_text(json.dumps(payload))
"#,
    )
    .expect("write fake bridge");

    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(&fixture)
        .arg("--json")
        .arg("--solver-timeout")
        .arg("50")
        .arg("--escalate-lean")
        .arg("--emit")
        .arg("sarif")
        .arg("--report-dir")
        .arg(&reports)
        .env("MUMEI_LEAN_PATH", &bridge_repo)
        .current_dir(&root)
        .output()
        .expect("run mumei with fake Lean bridge");
    assert_eq!(
        output.status.code(),
        Some(3),
        "rejected axiom audit remains inconclusive\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let document = read_sarif(&reports.join("report.sarif"));
    let rejected = sarif_result(
        document["runs"][0]["results"].as_array().unwrap(),
        "fermat3",
        "axiom_rejected",
    );
    assert_eq!(rejected["level"], "warning");
    assert_eq!(rejected["properties"]["obligation"], "lean_proof");
    assert_eq!(
        rejected["properties"]["disallowed_axioms"],
        serde_json::json!(["sorryAx"])
    );
    assert_eq!(
        document["runs"][0]["invocations"][0]["exitCode"],
        output.status.code().unwrap()
    );
    std::fs::remove_dir_all(root).expect("remove fixture directory");
}
