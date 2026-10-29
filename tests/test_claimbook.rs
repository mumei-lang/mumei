use mumei_core::proof_cert::{
    refresh_certificate_integrity, status, LeanResultMetadata, ProofCertificate,
};
use mumei_core::verification;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn fresh_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "mumei_claimbook_{label}_{}_{}_{}",
        std::process::id(),
        nonce,
        DIR_COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).expect("create test directory");
    dir
}

fn copy_fixture(dir: &Path, fixture: &str, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture),
        &path,
    )
    .expect("copy fixture into isolated directory");
    path
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run mumei")
}

fn certify(dir: &Path, source: &Path) -> PathBuf {
    let output = run_in(
        dir,
        &[
            "verify",
            "--proof-cert",
            source.to_str().expect("source path is UTF-8"),
        ],
    );
    let cert = source.with_extension("proof.json");
    assert!(
        cert.exists(),
        "verify did not write the default certificate\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    cert
}

fn claims(dir: &Path, source: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .arg("claims")
        .arg(source)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run mumei claims")
}

fn json_report(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read claimbook JSON"))
        .expect("parse claimbook JSON")
}

fn files_under(root: &Path) -> BTreeSet<PathBuf> {
    fn collect(root: &Path, current: &Path, files: &mut BTreeSet<PathBuf>) {
        for entry in std::fs::read_dir(current).expect("read directory") {
            let path = entry.expect("read directory entry").path();
            if path.is_dir() {
                collect(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root)
                        .expect("path under root")
                        .to_path_buf(),
                );
            }
        }
    }

    let mut files = BTreeSet::new();
    collect(root, root, &mut files);
    files
}

fn atom<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["claims"]
        .as_array()
        .expect("claims array")
        .iter()
        .find(|claim| claim["atom"] == name)
        .unwrap_or_else(|| panic!("claim {name:?} missing from {report}"))
}

fn without_discrimination(mut report: Value) -> Value {
    for claim in report["claims"].as_array_mut().expect("claims array") {
        claim
            .as_object_mut()
            .expect("claim object")
            .remove("discrimination");
    }
    report["summary"]
        .as_object_mut()
        .expect("summary object")
        .remove("by_discrimination");
    report
}

fn write_source(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write source fixture");
    path
}

fn write_harvest(dir: &Path, name: &str, target: &str, result: i64) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schema": "mumei.test-harvest/v1",
            "observations": [{
                "kind": "example",
                "target": target,
                "test": "observed_test",
                "file": "tests/test_claimbook.py",
                "line": 1,
                "args": {"x": 3},
                "result": result
            }]
        }))
        .expect("serialize harvest"),
    )
    .expect("write harvest");
    path
}

#[test]
fn controls_disabled_outputs_match_develop_goldens() {
    let dir = fresh_dir("controls_off_golden");
    copy_fixture(&dir, "claimbook_atoms.mm", "main.mm");
    let out_dir = dir.join("report");
    let out_dir_arg = out_dir.to_string_lossy().into_owned();
    let output = claims(
        &dir,
        Path::new("main.mm"),
        &["--format", "both", "--out-dir", &out_dir_arg],
    );
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let expected_json = include_bytes!("fixtures/claimbook_controls_off.json");
    assert_eq!(
        std::fs::read(out_dir.join("claimbook.json")).expect("read JSON"),
        expected_json
            .strip_suffix(b"\n")
            .expect("golden JSON has a repository-required trailing newline")
    );
    assert_eq!(
        std::fs::read(out_dir.join("CLAIMBOOK.md")).expect("read Markdown"),
        include_bytes!("fixtures/claimbook_controls_off.md")
    );
}

#[test]
fn controls_results_attach_to_claims_and_render_summary() {
    let dir = fresh_dir("controls_mixed");
    let source = write_source(
        &dir,
        "mixed.mm",
        "atom strong(x: i64, y: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x + y;\n\
         body: x + y;\n\
         atom weak(x: i64, y: i64) -> i64\n\
         requires: x >= 0 && y >= 0;\n\
         ensures: result >= 0;\n\
         body: x + y + 1;\n",
    );
    let plain_out_dir = dir.join("plain");
    let plain_out_dir_arg = plain_out_dir.to_string_lossy().into_owned();
    let plain_output = claims(
        &dir,
        &source,
        &["--format", "json", "--out-dir", &plain_out_dir_arg],
    );
    assert!(
        plain_output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&plain_output.stdout),
        String::from_utf8_lossy(&plain_output.stderr)
    );
    let plain_report = json_report(&plain_out_dir.join("claimbook.json"));

    let out_dir = dir.join("report");
    let out_dir_arg = out_dir.to_string_lossy().into_owned();
    let output = claims(
        &dir,
        &source,
        &["--controls", "--format", "both", "--out-dir", &out_dir_arg],
    );
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let report = json_report(&out_dir.join("claimbook.json"));
    assert_eq!(without_discrimination(report.clone()), plain_report);
    let strong = atom(&report, "strong")["discrimination"].clone();
    assert_eq!(strong["status"], "discriminating");
    assert_eq!(strong["survived"], 0);
    assert!(strong["killed"].as_u64().unwrap() > 0);
    let weak = atom(&report, "weak")["discrimination"].clone();
    assert_eq!(weak["status"], "weak");
    assert!(weak["surviving"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == "constant:1->0@2"));
    assert_eq!(
        report["summary"]["by_discrimination"],
        serde_json::json!({"discriminating": 1, "weak": 1})
    );

    let markdown = std::fs::read_to_string(out_dir.join("CLAIMBOOK.md")).expect("read Markdown");
    assert!(markdown.contains("Controls: discriminating=1, weak=1"));
    assert!(markdown.contains("| Controls |"));
    assert!(markdown.contains("discriminating 1/1"));
    assert!(markdown.contains("weak (1 survived)"));
}

#[test]
fn require_discriminating_gates_weak_and_accepts_strong_claims() {
    let weak_dir = fresh_dir("controls_gate_weak");
    let weak_source = write_source(
        &weak_dir,
        "weak.mm",
        "atom add(x: i64, y: i64) -> i64\n\
         requires: x >= 0 && y >= 0;\n\
         ensures: result >= 0;\n\
         body: x + y + 1;\n",
    );
    let weak_out = weak_dir.join("report");
    let weak_out_arg = weak_out.to_string_lossy().into_owned();
    let weak = claims(
        &weak_dir,
        &weak_source,
        &[
            "--require-discriminating",
            "--format",
            "both",
            "--out-dir",
            &weak_out_arg,
        ],
    );
    assert_eq!(weak.status.code(), Some(1));
    assert!(weak_out.join("claimbook.json").exists());
    assert!(weak_out.join("CLAIMBOOK.md").exists());
    let weak_stderr = String::from_utf8_lossy(&weak.stderr);
    assert!(weak_stderr.contains("add: controls weak"), "{weak_stderr}");
    assert!(weak_stderr.contains("constant:1->0@2"), "{weak_stderr}");

    let strong_dir = fresh_dir("controls_gate_strong");
    let strong_source = write_source(
        &strong_dir,
        "strong.mm",
        "atom add(x: i64, y: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x + y;\n\
         body: x + y;\n",
    );
    let strong = claims(
        &strong_dir,
        &strong_source,
        &["--require-discriminating", "--format", "json"],
    );
    assert_eq!(
        strong.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&strong.stderr)
    );
}

#[test]
fn trusted_and_method_claims_attach_controls_by_name() {
    let trusted_dir = fresh_dir("controls_trusted");
    let trusted_source = write_source(
        &trusted_dir,
        "trusted.mm",
        "trusted atom external() -> i64\n\
         requires: true;\n\
         ensures: result == 7;\n\
         body: 7;\n",
    );
    let trusted_out = trusted_dir.join("report");
    let trusted_out_arg = trusted_out.to_string_lossy().into_owned();
    let trusted = claims(
        &trusted_dir,
        &trusted_source,
        &[
            "--require-discriminating",
            "--format",
            "json",
            "--out-dir",
            &trusted_out_arg,
        ],
    );
    assert_eq!(trusted.status.code(), Some(0));
    let trusted_report = json_report(&trusted_out.join("claimbook.json"));
    assert_eq!(
        atom(&trusted_report, "external")["discrimination"]["status"],
        "not_checked"
    );

    let method_dir = fresh_dir("controls_method");
    let method_source = copy_fixture(&method_dir, "claimbook_impl.mm", "impl.mm");
    let method_out = method_dir.join("report");
    let method_out_arg = method_out.to_string_lossy().into_owned();
    let method = claims(
        &method_dir,
        &method_source,
        &[
            "--controls",
            "--format",
            "json",
            "--out-dir",
            &method_out_arg,
        ],
    );
    assert_eq!(method.status.code(), Some(0));
    let method_report = json_report(&method_out.join("claimbook.json"));
    assert_ne!(
        atom(&method_report, "Point::get")["discrimination"]["status"],
        Value::Null
    );
}

#[test]
fn generic_instances_do_not_add_claims_when_controls_are_enabled() {
    let dir = fresh_dir("controls_generic_instance");
    let source = write_source(
        &dir,
        "generic.mm",
        "atom id<T>(x: T) -> T\n\
         requires: true;\n\
         ensures: result == x + 1;\n\
         body: x + 1;\n\
         atom main() -> i64\n\
         requires: true;\n\
         ensures: result == 4;\n\
         body: id<i64>(3);\n",
    );
    let plain_out_dir = dir.join("plain");
    let plain_out_dir_arg = plain_out_dir.to_string_lossy().into_owned();
    let plain = claims(
        &dir,
        &source,
        &["--format", "json", "--out-dir", &plain_out_dir_arg],
    );
    assert!(
        plain.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&plain.stdout),
        String::from_utf8_lossy(&plain.stderr)
    );

    let controls_out_dir = dir.join("controls");
    let controls_out_dir_arg = controls_out_dir.to_string_lossy().into_owned();
    let with_controls = claims(
        &dir,
        &source,
        &[
            "--controls",
            "--format",
            "json",
            "--out-dir",
            &controls_out_dir_arg,
        ],
    );
    assert!(
        with_controls.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&with_controls.stdout),
        String::from_utf8_lossy(&with_controls.stderr)
    );

    let plain_report = json_report(&plain_out_dir.join("claimbook.json"));
    let controls_report = json_report(&controls_out_dir.join("claimbook.json"));
    let claim_names = |report: &Value| {
        report["claims"]
            .as_array()
            .expect("claims array")
            .iter()
            .map(|claim| claim["atom"].as_str().expect("claim name").to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(claim_names(&controls_report), claim_names(&plain_report));
    assert_eq!(
        controls_report["summary"]["total"],
        plain_report["summary"]["total"]
    );
    assert_eq!(
        controls_report["summary"]["by_effective_grade"],
        plain_report["summary"]["by_effective_grade"]
    );
}

#[test]
fn discrimination_gate_combines_with_grade_gate_without_rejecting_trusted_controls() {
    let dir = fresh_dir("controls_grade_gate");
    let source = write_source(
        &dir,
        "trusted_helper.mm",
        "trusted atom trusted_helper() -> i64\n\
         requires: true;\n\
         ensures: result == 7;\n\
         body: 7;\n\
         atom caller() -> i64\n\
         requires: true;\n\
         ensures: result == 7;\n\
         body: trusted_helper();\n",
    );
    certify(&dir, &source);
    let out_dir = dir.join("report");
    let out_dir_arg = out_dir.to_string_lossy().into_owned();
    let output = claims(
        &dir,
        &source,
        &[
            "--require-discriminating",
            "--require-grade",
            "G4",
            "--format",
            "both",
            "--out-dir",
            &out_dir_arg,
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(out_dir.join("claimbook.json").exists());
    assert!(out_dir.join("CLAIMBOOK.md").exists());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("trusted_helper: effective G1"), "{stderr}");
    assert!(!stderr.contains("controls "), "{stderr}");
}

#[test]
fn max_mutants_caps_controls_and_invalid_usage_is_rejected() {
    let dir = fresh_dir("controls_cap");
    let source = write_source(
        &dir,
        "input.mm",
        "atom add(x: i64, y: i64) -> i64\n\
         requires: x >= 0 && y >= 0;\n\
         ensures: result >= 0;\n\
         body: x + y + 1;\n",
    );
    let out_dir = dir.join("report");
    let out_dir_arg = out_dir.to_string_lossy().into_owned();
    let capped = claims(
        &dir,
        &source,
        &[
            "--controls",
            "--max-mutants",
            "1",
            "--format",
            "json",
            "--out-dir",
            &out_dir_arg,
        ],
    );
    assert_eq!(capped.status.code(), Some(0));
    assert_eq!(
        atom(&json_report(&out_dir.join("claimbook.json")), "add")["discrimination"]["tested"],
        1
    );

    let zero = claims(&dir, &source, &["--controls", "--max-mutants", "0"]);
    assert_eq!(zero.status.code(), Some(4));

    let missing_controls = claims(&dir, &source, &["--max-mutants", "1"]);
    assert_eq!(missing_controls.status.code(), Some(2));
}

#[test]
fn controls_reports_are_deterministic_and_read_only() {
    let dir = fresh_dir("controls_read_only");
    let source = write_source(
        &dir,
        "input.mm",
        "atom add(x: i64, y: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x + y;\n\
         body: x + y;\n",
    );
    let out_dir = dir.join("report");
    let out_dir_arg = out_dir.to_string_lossy().into_owned();
    let before = files_under(&dir);
    let first = claims(
        &dir,
        &source,
        &["--controls", "--format", "both", "--out-dir", &out_dir_arg],
    );
    assert!(
        first.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    let first_json = std::fs::read(out_dir.join("claimbook.json")).expect("read first JSON report");
    let first_markdown =
        std::fs::read(out_dir.join("CLAIMBOOK.md")).expect("read first Markdown report");
    let second = claims(
        &dir,
        &source,
        &["--controls", "--format", "both", "--out-dir", &out_dir_arg],
    );
    assert!(
        second.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(!dir.join(".mumei_cache").exists());
    assert_eq!(
        std::fs::read(out_dir.join("claimbook.json")).expect("read second JSON report"),
        first_json
    );
    assert_eq!(
        std::fs::read(out_dir.join("CLAIMBOOK.md")).expect("read second Markdown report"),
        first_markdown
    );
    assert_eq!(
        files_under(&dir)
            .difference(&before)
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            PathBuf::from("report/claimbook.json"),
            PathBuf::from("report/CLAIMBOOK.md"),
        ])
    );
}

#[test]
fn certified_atoms_have_grades_freshness_and_deterministic_json() {
    let dir = fresh_dir("grades");
    let source = copy_fixture(&dir, "claimbook_atoms.mm", "main.mm");
    let cert = certify(&dir, &source);
    assert!(cert.exists());
    let generated_cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&cert).expect("read generated certificate"))
            .expect("parse generated certificate");
    let trusted_cert = generated_cert
        .atoms
        .iter()
        .find(|atom| atom.name == "trusted_value")
        .expect("trusted fixture certificate entry");
    assert_eq!(trusted_cert.status, status::TRUSTED);
    assert_eq!(trusted_cert.z3_check_result, status::Z3_SKIPPED);

    let out_dir = dir.join("report");
    let output_dir_arg = out_dir.to_string_lossy().into_owned();
    let output = claims(
        &dir,
        &source,
        &["--format", "both", "--out-dir", &output_dir_arg],
    );
    assert!(
        output.status.success(),
        "claims failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let json_path = out_dir.join("claimbook.json");
    let markdown_path = out_dir.join("CLAIMBOOK.md");
    let report = json_report(&json_path);
    assert_eq!(report["schema"], "mumei.claimbook/v1");
    assert_eq!(atom(&report, "verified_value")["grade"], "G4");
    assert_eq!(
        atom(&report, "verified_value")["grade_label"],
        "machine-sealed"
    );
    assert_eq!(atom(&report, "verified_value")["freshness"], "fresh");
    assert_eq!(atom(&report, "trusted_value")["grade"], "G1");
    assert_eq!(atom(&report, "trusted_value")["freshness"], "fresh");
    assert_eq!(atom(&report, "trusted_value")["evidence_kind"], "trusted");
    assert_eq!(
        atom(&report, "trusted_value")["reason"],
        "the contract is marked trusted; its body is not checked"
    );
    assert_eq!(atom(&report, "failing_value")["grade"], "G0");
    assert_eq!(atom(&report, "failing_value")["freshness"], "fresh");
    let names = report["claims"]
        .as_array()
        .expect("claims array")
        .iter()
        .map(|claim| claim["atom"].as_str().expect("atom string"))
        .collect::<Vec<_>>();
    assert_eq!(names, ["failing_value", "trusted_value", "verified_value"]);
    assert!(std::fs::read_to_string(&markdown_path)
        .expect("read markdown")
        .contains("# Claim Book"));

    let first_bytes = std::fs::read(&json_path).expect("read first JSON");
    let cert_arg = cert.to_string_lossy().into_owned();
    let repeated = claims(
        &dir,
        &source,
        &[
            "--cert",
            &cert_arg,
            "--format",
            "json",
            "--out-dir",
            &output_dir_arg,
        ],
    );
    assert!(repeated.status.success());
    assert_eq!(
        first_bytes,
        std::fs::read(json_path).expect("read repeated JSON")
    );
}

#[test]
fn source_edits_new_atoms_removed_atoms_and_certificate_edits_are_reported() {
    let dir = fresh_dir("freshness");
    let source = copy_fixture(&dir, "claimbook_atoms.mm", "main.mm");
    let cert_path = certify(&dir, &source);
    let out_dir = dir.join("report");
    let out_dir_arg = out_dir.to_string_lossy().into_owned();

    let initial = std::fs::read_to_string(&source).expect("read source");
    let initial_report = claims(
        &dir,
        &source,
        &["--format", "json", "--out-dir", &out_dir_arg],
    );
    assert!(initial_report.status.success());
    let initial_hash = atom(
        &json_report(&out_dir.join("claimbook.json")),
        "verified_value",
    )["content_hash"]
        .clone();

    let comment_and_whitespace = format!(
        "// formatting-only change\n\n{}",
        initial.replace("  requires:", "    requires:")
    );
    std::fs::write(&source, &comment_and_whitespace).expect("write comment and whitespace edit");
    let output = claims(
        &dir,
        &source,
        &["--format", "json", "--out-dir", &out_dir_arg],
    );
    assert!(output.status.success());
    let report = json_report(&out_dir.join("claimbook.json"));
    assert_eq!(atom(&report, "verified_value")["freshness"], "fresh");
    assert_eq!(
        atom(&report, "verified_value")["content_hash"],
        initial_hash,
        "comment and indentation-only edits preserve the content hash"
    );

    let edited = initial.replacen(
        "ensures: result == x;\n  body: x;",
        "ensures: result >= x;\n  body: x;",
        1,
    );
    assert_ne!(initial, edited);
    std::fs::write(&source, &edited).expect("edit verified source");
    let output = claims(
        &dir,
        &source,
        &["--format", "json", "--out-dir", &out_dir_arg],
    );
    assert!(output.status.success());
    let report = json_report(&out_dir.join("claimbook.json"));
    assert_eq!(atom(&report, "verified_value")["freshness"], "drifted");
    assert_eq!(atom(&report, "verified_value")["effective_grade"], "G0");
    assert_eq!(atom(&report, "trusted_value")["freshness"], "fresh");

    std::fs::write(&source, &initial).expect("restore original source");
    let with_new_atom = format!(
        "{initial}\natom added_later() -> i64\n  requires: true;\n  ensures: result == 7;\n  body: 7;\n"
    );
    std::fs::write(&source, &with_new_atom).expect("add atom without certification");
    let output = claims(
        &dir,
        &source,
        &["--format", "json", "--out-dir", &out_dir_arg],
    );
    assert!(output.status.success());
    let report = json_report(&out_dir.join("claimbook.json"));
    assert_eq!(atom(&report, "added_later")["freshness"], "uncertified");
    assert_eq!(atom(&report, "added_later")["effective_grade"], "G0");

    let without_failed = with_new_atom.replace(
        "atom failing_value(x: i64) -> i64\n  requires: true;\n  ensures: result == x + 1;\n  body: x;\n",
        "",
    );
    std::fs::write(&source, without_failed).expect("remove certified atom");
    let output = claims(
        &dir,
        &source,
        &["--format", "json", "--out-dir", &out_dir_arg],
    );
    assert!(output.status.success());
    let report = json_report(&out_dir.join("claimbook.json"));
    assert!(report["orphaned_entries"]
        .as_array()
        .expect("orphaned entries")
        .iter()
        .any(|entry| entry["atom"] == "failing_value"));

    let mut cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&cert_path).expect("read generated certificate"))
            .expect("parse generated certificate");
    cert.atoms[0].status = status::FAILED.to_string();
    std::fs::write(
        &cert_path,
        serde_json::to_vec_pretty(&cert).expect("serialize modified certificate"),
    )
    .expect("write modified certificate");
    std::fs::write(&source, &initial).expect("restore source before integrity check");
    let output = claims(
        &dir,
        &source,
        &["--format", "json", "--out-dir", &out_dir_arg],
    );
    assert!(output.status.success());
    let report = json_report(&out_dir.join("claimbook.json"));
    assert_eq!(report["certificate_integrity"], "mismatch");
    assert!(report["claims"]
        .as_array()
        .expect("claims array")
        .iter()
        .all(|claim| claim["freshness"] == "drifted"));
}

#[test]
fn formats_output_directories_and_missing_certificates_are_handled() {
    let dir = fresh_dir("formats");
    let source = copy_fixture(&dir, "claimbook_atoms.mm", "main.mm");
    assert!(!source.with_extension("proof.json").exists());
    let json_dir = dir.join("json-output");
    let json_dir_arg = json_dir.to_string_lossy().into_owned();
    let json_output = claims(
        &dir,
        &source,
        &["--format", "json", "--out-dir", &json_dir_arg],
    );
    assert!(json_output.status.success());
    assert!(json_dir.join("claimbook.json").exists());
    assert!(!json_dir.join("CLAIMBOOK.md").exists());
    let missing_cert_report = json_report(&json_dir.join("claimbook.json"));
    let missing_claims = missing_cert_report["claims"]
        .as_array()
        .expect("claims array");
    assert!(missing_claims.iter().all(|claim| {
        claim["freshness"] == "uncertified"
            && claim["grade"] == "G0"
            && claim["effective_grade"] == "G0"
    }));

    let md_dir = dir.join("md-output");
    let md_dir_arg = md_dir.to_string_lossy().into_owned();
    let md_output = claims(&dir, &source, &["--format", "md", "--out-dir", &md_dir_arg]);
    assert!(md_output.status.success());
    assert!(md_dir.join("CLAIMBOOK.md").exists());
    assert!(!md_dir.join("claimbook.json").exists());

    let both_dir = dir.join("both-output");
    let both_dir_arg = both_dir.to_string_lossy().into_owned();
    let both_output = claims(
        &dir,
        &source,
        &["--format", "both", "--out-dir", &both_dir_arg],
    );
    assert!(both_output.status.success());
    assert!(both_dir.join("CLAIMBOOK.md").exists());
    assert!(both_dir.join("claimbook.json").exists());
}

#[test]
fn observations_lift_g2_gate_claims_and_reject_invalid_harvests() {
    let dir = fresh_dir("observations");
    let source = write_source(
        &dir,
        "observed.mm",
        "atom identity(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    );
    let harvest = write_harvest(&dir, "consistent.json", "identity", 3);
    let harvest_arg = harvest.to_string_lossy().into_owned();
    let output_dir = dir.join("observed-report");
    let output_dir_arg = output_dir.to_string_lossy().into_owned();
    let output = claims(
        &dir,
        &source,
        &[
            "--observations",
            &harvest_arg,
            "--require-grade",
            "G2",
            "--format",
            "json",
            "--out-dir",
            &output_dir_arg,
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = json_report(&output_dir.join("claimbook.json"));
    let identity = atom(&report, "identity");
    assert_eq!(identity["grade"], "G2");
    assert_eq!(identity["effective_grade"], "G2");
    assert_eq!(identity["evidence_kind"], "observation");
    assert_eq!(
        identity["reason"],
        "current test observations agree with the contract"
    );
    assert_eq!(identity["observation"]["status"], "supported");
    assert_eq!(report["summary"]["by_observation"]["supported"], 1);

    let markdown_dir = dir.join("markdown-report");
    let markdown_dir_arg = markdown_dir.to_string_lossy().into_owned();
    let markdown = claims(
        &dir,
        &source,
        &[
            "--observations",
            &harvest_arg,
            "--format",
            "md",
            "--out-dir",
            &markdown_dir_arg,
        ],
    );
    assert!(markdown.status.success());
    let markdown = std::fs::read_to_string(markdown_dir.join("CLAIMBOOK.md"))
        .expect("read Claim Book Markdown");
    assert!(markdown.contains(
        "| Atom | Own grade | Freshness | Evidence | Reason | Limiting premises | Observation |"
    ));
    assert!(markdown.contains("| supported |"));
    assert!(markdown.contains("| Observation | Count |"));

    let contradicting_harvest = write_harvest(&dir, "contradicting.json", "identity", 5);
    let contradicting_harvest_arg = contradicting_harvest.to_string_lossy().into_owned();
    let rejected_dir = dir.join("rejected-report");
    let rejected_dir_arg = rejected_dir.to_string_lossy().into_owned();
    let rejected = claims(
        &dir,
        &source,
        &[
            "--observations",
            &contradicting_harvest_arg,
            "--require-grade",
            "G2",
            "--format",
            "json",
            "--out-dir",
            &rejected_dir_arg,
        ],
    );
    assert_eq!(rejected.status.code(), Some(1));
    let rejected_report = json_report(&rejected_dir.join("claimbook.json"));
    let identity = atom(&rejected_report, "identity");
    assert_eq!(identity["grade"], "G0");
    assert_eq!(identity["observation"]["status"], "contradicted");

    let missing_harvest = dir.join("missing.json");
    let missing_harvest_arg = missing_harvest.to_string_lossy().into_owned();
    let missing = claims(&dir, &source, &["--observations", &missing_harvest_arg]);
    assert_eq!(missing.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("❌"));

    let invalid_harvest = dir.join("invalid.json");
    std::fs::write(&invalid_harvest, "{").expect("write invalid harvest");
    let invalid_harvest_arg = invalid_harvest.to_string_lossy().into_owned();
    let invalid = claims(&dir, &source, &["--observations", &invalid_harvest_arg]);
    assert_eq!(invalid.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("❌"));
}

#[test]
fn uncertified_caller_observation_respects_unobserved_callee_grade_gate() {
    let dir = fresh_dir("observed_dependency");
    let source = write_source(
        &dir,
        "dependency.mm",
        "atom callee(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n\
         atom caller(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: callee(x);\n",
    );
    let harvest = write_harvest(&dir, "caller.json", "caller", 3);
    let harvest_arg = harvest.to_string_lossy().into_owned();
    let output_dir = dir.join("report");
    let output_dir_arg = output_dir.to_string_lossy().into_owned();
    let output = claims(
        &dir,
        &source,
        &[
            "--observations",
            &harvest_arg,
            "--require-grade",
            "G2",
            "--format",
            "json",
            "--out-dir",
            &output_dir_arg,
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = json_report(&output_dir.join("claimbook.json"));
    let caller = atom(&report, "caller");
    assert_eq!(caller["grade"], "G2");
    assert_eq!(caller["effective_grade"], "G0");
    assert_eq!(caller["observation"]["status"], "supported");
    assert_eq!(caller["dependencies"], serde_json::json!(["callee"]));
    assert_eq!(
        caller["limiting_premises"],
        serde_json::json!([{
            "kind": "dependency",
            "name": "callee",
            "grade": "G0"
        }])
    );
    let callee = atom(&report, "callee");
    assert_eq!(callee["grade"], "G0");
    assert_eq!(callee["effective_grade"], "G0");
}

#[test]
fn an_explicit_missing_certificate_returns_exit_four() {
    let dir = fresh_dir("missing_explicit_cert");
    let source = copy_fixture(&dir, "claimbook_atoms.mm", "main.mm");
    let missing_cert = dir.join("missing.proof.json");
    let missing_cert_arg = missing_cert.to_string_lossy().into_owned();
    let output = claims(&dir, &source, &["--cert", &missing_cert_arg]);

    assert_eq!(
        output.status.code(),
        Some(4),
        "claims should fail for an explicit missing certificate\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains(&format!(
        "❌ Proof certificate not found: {missing_cert_arg}"
    )));
}

#[test]
fn dependency_and_assumption_evidence_propagate_and_grade_gate_writes_reports() {
    let dir = fresh_dir("propagation");
    let source = copy_fixture(&dir, "claimbook_propagation.mm", "main.mm");
    let cert_path = certify(&dir, &source);
    let cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&cert_path).expect("read generated certificate"))
            .expect("parse generated certificate");
    let caller_certificate = cert
        .atoms
        .iter()
        .find(|entry| entry.name == "trusted_caller")
        .expect("caller certificate");
    assert_eq!(
        caller_certificate.dependencies,
        vec!["trusted_leaf".to_string()]
    );

    let gate_g4_dir = dir.join("gate-g4");
    let gate_g4_dir_arg = gate_g4_dir.to_string_lossy().into_owned();
    let gate_g4 = claims(
        &dir,
        &source,
        &[
            "--require-grade",
            "G4",
            "--format",
            "both",
            "--out-dir",
            &gate_g4_dir_arg,
        ],
    );
    assert_eq!(gate_g4.status.code(), Some(1));
    assert!(gate_g4_dir.join("claimbook.json").exists());
    assert!(gate_g4_dir.join("CLAIMBOOK.md").exists());
    let report = json_report(&gate_g4_dir.join("claimbook.json"));
    assert_eq!(atom(&report, "trusted_caller")["effective_grade"], "G1");
    assert_eq!(
        atom(&report, "trusted_caller")["limiting_premises"],
        serde_json::json!([{
            "kind": "dependency",
            "name": "trusted_leaf",
            "grade": "G1"
        }])
    );
    assert!(atom(&report, "trusted_caller")["limiting_premises"][0]
        .get("matched_from")
        .is_none());
    assert_eq!(atom(&report, "assumed_contract")["effective_grade"], "G1");
    assert_eq!(
        atom(&report, "assumed_contract")["limiting_premises"],
        serde_json::json!([{
            "kind": "assumed_clause",
            "name": "x > 0",
            "grade": "G1"
        }])
    );
    assert_eq!(
        report["trusted_base"]["trusted_atoms"],
        serde_json::json!(["trusted_leaf"])
    );
    assert_eq!(
        report["trusted_base"]["assumed_clauses"],
        serde_json::json!([{
            "atom": "assumed_contract",
            "clause": "requires: x > 0"
        }])
    );
    let gate_error = String::from_utf8_lossy(&gate_g4.stderr);
    assert!(gate_error.contains("trusted_leaf: effective G1 (vouched), required G4; limiting: -"));
    assert!(gate_error.contains(
        "trusted_caller: effective G1 (vouched), required G4; limiting: dependency trusted_leaf (G1)"
    ));
    assert!(gate_error.contains(
        "assumed_contract: effective G1 (vouched), required G4; limiting: assumed_clause x > 0 (G1)"
    ));

    for required_grade in ["G1", "G0"] {
        let out_dir = dir.join(format!("gate-{required_grade}"));
        let out_dir_arg = out_dir.to_string_lossy().into_owned();
        let output = claims(
            &dir,
            &source,
            &[
                "--require-grade",
                required_grade,
                "--format",
                "json",
                "--out-dir",
                &out_dir_arg,
            ],
        );
        assert!(
            output.status.success(),
            "grade gate {required_grade} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(out_dir.join("claimbook.json").exists());
    }

    let invalid_grade = claims(&dir, &source, &["--require-grade", "G6"]);
    assert_eq!(invalid_grade.status.code(), Some(2));
}

#[test]
fn incomparable_certificate_integrity_does_not_downgrade_current_evidence() {
    let dir = fresh_dir("incomparable");
    let source = copy_fixture(&dir, "claimbook_atoms.mm", "main.mm");
    let cert_path = certify(&dir, &source);
    let mut cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&cert_path).expect("read generated certificate"))
            .expect("parse generated certificate");
    cert.mumei_version = "another-version".to_string();
    std::fs::write(
        &cert_path,
        serde_json::to_vec_pretty(&cert).expect("serialize version-mismatched certificate"),
    )
    .expect("write version-mismatched certificate");

    let out_dir = dir.join("report");
    let out_dir_arg = out_dir.to_string_lossy().into_owned();
    let output = claims(
        &dir,
        &source,
        &["--format", "json", "--out-dir", &out_dir_arg],
    );
    assert!(output.status.success());
    let report = json_report(&out_dir.join("claimbook.json"));
    assert_eq!(report["certificate_integrity"], "incomparable_version");
    assert_eq!(atom(&report, "verified_value")["freshness"], "fresh");
    assert_eq!(atom(&report, "verified_value")["effective_grade"], "G4");
    assert_eq!(atom(&report, "trusted_value")["effective_grade"], "G1");
}

#[test]
fn impl_methods_use_qualified_atom_names() {
    let dir = fresh_dir("impl");
    let source = copy_fixture(&dir, "claimbook_impl.mm", "impl.mm");
    let out_dir = dir.join("report");
    let out_dir_arg = out_dir.to_string_lossy().into_owned();
    let output = claims(
        &dir,
        &source,
        &["--format", "json", "--out-dir", &out_dir_arg],
    );
    assert!(output.status.success());
    let report = json_report(&out_dir.join("claimbook.json"));
    assert_eq!(report["claims"].as_array().expect("claims array").len(), 1);
    assert_eq!(report["claims"][0]["atom"], "Point::get");
}

#[test]
fn method_name_dependency_keeps_qualified_premise_name() {
    let dir = fresh_dir("method-name-premise");
    let source = copy_fixture(&dir, "claimbook_impl.mm", "impl.mm");
    let contents = std::fs::read_to_string(&source)
        .expect("read fixture")
        .replace("    atom get(", "    trusted atom get(");
    std::fs::write(
        &source,
        format!(
            "{contents}\natom caller() -> i64\n  requires: true;\n  ensures: result == 7;\n  body: 7;\n"
        ),
    )
    .expect("write method dependency source");
    let cert_path = certify(&dir, &source);
    let mut cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&cert_path).expect("read generated certificate"))
            .expect("parse generated certificate");
    cert.atoms
        .iter_mut()
        .find(|entry| entry.name == "caller")
        .expect("caller certificate")
        .dependencies = vec!["get".to_string()];
    refresh_certificate_integrity(&mut cert);
    std::fs::write(
        &cert_path,
        serde_json::to_vec_pretty(&cert).expect("serialize method dependency certificate"),
    )
    .expect("write method dependency certificate");

    let out_dir = dir.join("report");
    let out_dir_arg = out_dir.to_string_lossy().into_owned();
    let output = claims(
        &dir,
        &source,
        &["--format", "both", "--out-dir", &out_dir_arg],
    );
    assert!(
        output.status.success(),
        "claims failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = json_report(&out_dir.join("claimbook.json"));
    let caller = atom(&report, "caller");
    assert_eq!(caller["dependencies"], serde_json::json!(["Point::get"]));
    assert_eq!(
        caller["limiting_premises"],
        serde_json::json!([{
            "kind": "dependency",
            "name": "Point::get",
            "matched_from": "get",
            "grade": "G1"
        }])
    );
    assert_eq!(caller["effective_grade_complete"], false);
    let markdown =
        std::fs::read_to_string(out_dir.join("CLAIMBOOK.md")).expect("read Claim Book markdown");
    assert!(markdown.contains("dependency `Point::get` (matched from `get` by method name) (G1)"));

    let gate = claims(
        &dir,
        &source,
        &[
            "--require-grade",
            "G4",
            "--format",
            "json",
            "--out-dir",
            &out_dir_arg,
        ],
    );
    assert_eq!(gate.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&gate.stderr)
        .contains("dependency `Point::get` (matched from `get` by method name) (G1)"));
}

#[test]
fn lean_verified_evidence_requires_opt_in_and_current_audit() {
    let dir = fresh_dir("lean");
    let source = copy_fixture(&dir, "claimbook_atoms.mm", "main.mm");
    let cert_path = certify(&dir, &source);
    let mut cert: ProofCertificate =
        serde_json::from_slice(&std::fs::read(&cert_path).expect("read generated certificate"))
            .expect("parse generated certificate");
    let atom_cert = cert
        .atoms
        .iter_mut()
        .find(|atom| atom.name == "verified_value")
        .expect("verified atom certificate");
    atom_cert.z3_check_result = status::Z3_LEAN_VERIFIED.to_string();
    atom_cert.translator_version = verification::LEAN_TRANSLATOR_VERSION.to_string();
    atom_cert.bridge_lemma_hash = verification::LEAN_BRIDGE_LEMMA_HASH.to_string();
    atom_cert.lean_result_metadata = Some(LeanResultMetadata {
        status: status::LEAN_STATUS_VERIFIED.to_string(),
        theorem_name: "verified_value_spec".to_string(),
        translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
        bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
        proof_path: "Generated/ClaimBook.lean".to_string(),
        diagnostics: vec![],
        kernel_axioms: Some(vec!["propext".to_string()]),
        axiom_audit: Some("passed".to_string()),
        ..Default::default()
    });
    refresh_certificate_integrity(&mut cert);
    std::fs::write(
        &cert_path,
        serde_json::to_vec_pretty(&cert).expect("serialize lean certificate"),
    )
    .expect("write lean certificate");

    let out_dir = dir.join("report");
    let out_dir_arg = out_dir.to_string_lossy().into_owned();
    let without_opt_in = claims(
        &dir,
        &source,
        &["--format", "json", "--out-dir", &out_dir_arg],
    );
    assert!(without_opt_in.status.success());
    assert_eq!(
        atom(
            &json_report(&out_dir.join("claimbook.json")),
            "verified_value"
        )["grade"],
        "G0"
    );
    let with_opt_in = claims(
        &dir,
        &source,
        &[
            "--allow-lean-verified",
            "--format",
            "json",
            "--out-dir",
            &out_dir_arg,
        ],
    );
    assert!(with_opt_in.status.success());
    let report = json_report(&out_dir.join("claimbook.json"));
    let claim = atom(&report, "verified_value");
    assert_eq!(claim["grade"], "G4");
    assert_eq!(claim["evidence_kind"], "lean");
}

#[test]
fn claims_with_import_does_not_persist_resolver_cache_or_create_unrequested_files() {
    let dir = fresh_dir("read_only_import");
    let source = dir.join("main.mm");
    std::fs::write(
        &source,
        r#"
import "dependency" as dependency;

atom main_value(x: i64) -> i64 {
    requires: true;
    ensures: result == x;
    body: x
}
"#,
    )
    .expect("write source with import");
    std::fs::write(
        dir.join("dependency.mm"),
        r#"
atom dependency_value(x: i64) -> i64 {
    requires: true;
    ensures: result == x;
    body: x
}
"#,
    )
    .expect("write imported fixture");
    certify(&dir, &source);

    let cache_path = dir.join(".mumei_cache");
    assert!(
        cache_path.exists(),
        "verify should exercise resolver caching"
    );
    std::fs::remove_file(&cache_path).expect("remove resolver cache before claims");

    let out_dir = dir.join("report");
    let out_dir_arg = out_dir.to_string_lossy().into_owned();
    let before = files_under(&dir);
    let first = claims(
        &dir,
        &source,
        &["--controls", "--format", "both", "--out-dir", &out_dir_arg],
    );
    assert!(
        first.status.success(),
        "claims failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        !cache_path.exists(),
        "claims must not recreate .mumei_cache"
    );

    let json_path = PathBuf::from("report/claimbook.json");
    let markdown_path = PathBuf::from("report/CLAIMBOOK.md");
    let expected_new_files = BTreeSet::from([json_path.clone(), markdown_path.clone()]);
    let created = files_under(&dir)
        .difference(&before)
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(created, expected_new_files);
    let first_json = std::fs::read(dir.join(&json_path)).expect("read first JSON report");
    let first_markdown =
        std::fs::read(dir.join(&markdown_path)).expect("read first Markdown report");

    let second = claims(
        &dir,
        &source,
        &["--controls", "--format", "both", "--out-dir", &out_dir_arg],
    );
    assert!(
        second.status.success(),
        "second claims run failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        std::fs::read(dir.join(&json_path)).expect("read second JSON report"),
        first_json
    );
    assert_eq!(
        std::fs::read(dir.join(&markdown_path)).expect("read second Markdown report"),
        first_markdown
    );
    assert!(
        !cache_path.exists(),
        "claims must remain read-only on rerun"
    );
    assert_eq!(
        files_under(&dir)
            .difference(&before)
            .cloned()
            .collect::<BTreeSet<_>>(),
        expected_new_files
    );
}

#[test]
fn unparsable_certificate_and_input_return_exit_four() {
    let dir = fresh_dir("bad-cert");
    let source = copy_fixture(&dir, "claimbook_atoms.mm", "main.mm");
    let cert = dir.join("invalid.json");
    std::fs::write(&cert, "{").expect("write invalid certificate");
    let cert_arg = cert.to_string_lossy().into_owned();
    let output = claims(&dir, &source, &["--cert", &cert_arg]);
    assert_eq!(output.status.code(), Some(4));

    let invalid_source = dir.join("invalid.mm");
    std::fs::write(&invalid_source, "atom").expect("write invalid source");
    let output = claims(
        &dir,
        &invalid_source,
        &[
            "--format",
            "json",
            "--out-dir",
            dir.to_str().expect("UTF-8 path"),
        ],
    );
    assert_eq!(output.status.code(), Some(4));
}
