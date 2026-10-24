use mumei_core::proof_cert::{
    refresh_certificate_integrity, status, LeanResultMetadata, ProofCertificate,
};
use mumei_core::verification;
use serde_json::Value;
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

fn atom<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["claims"]
        .as_array()
        .expect("claims array")
        .iter()
        .find(|claim| claim["atom"] == name)
        .unwrap_or_else(|| panic!("claim {name:?} missing from {report}"))
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
            "clause": "x > 0"
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
