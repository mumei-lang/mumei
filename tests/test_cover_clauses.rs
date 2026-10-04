use mumei_core::parser::{parse_module, CoverClause, Item};
use mumei_core::proof_cert::compute_atom_content_hash_v2;
use mumei_core::resolver::compute_proof_hash;
use mumei_core::verification::ModuleEnv;
use std::path::PathBuf;
use std::process::{Command, Output};

fn verify_source(name: &str, source: &str) -> (PathBuf, Output, serde_json::Value, String) {
    let dir = std::env::temp_dir().join(format!(
        "mumei_cover_clauses_{}_{}_{}",
        std::process::id(),
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let fixture = dir.join(format!("{name}.mm"));
    let report_dir = dir.join("reports");
    std::fs::create_dir_all(&report_dir).expect("create report directory");
    std::fs::write(&fixture, source).expect("write fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--report-dir")
        .arg(&report_dir)
        .arg("--disable-spurious-detection")
        .arg(&fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify");
    let report: serde_json::Value = serde_json::from_slice(
        &std::fs::read(report_dir.join("report.json")).expect("read report.json"),
    )
    .expect("parse report.json");
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (dir, output, report, text)
}

fn is_odd_witness(value: &serde_json::Value) -> bool {
    let Some(value) = value.as_str() else {
        return false;
    };
    let parsed = if let Some(hex) = value.strip_prefix("#x") {
        u128::from_str_radix(hex, 16).ok()
    } else if let Some(binary) = value.strip_prefix("#b") {
        u128::from_str_radix(binary, 2).ok()
    } else if let Some(bitvector) = value.strip_prefix("(_ bv") {
        bitvector
            .split_whitespace()
            .next()
            .and_then(|number| number.parse::<u128>().ok())
    } else {
        value.parse::<i128>().ok().map(|integer| integer as u128)
    };
    parsed.is_some_and(|integer| integer & 1 == 1)
}

#[test]
fn cover_witness_is_reported_and_keeps_ensures_summary() {
    let (dir, output, report, text) = verify_source(
        "abs_val",
        r#"
atom abs_val(x: i64) -> i64
requires: x > -1000 && x < 1000;
ensures: result >= 0;
cover "zero": result == 0;
body: if x < 0 { 0 - x } else { x };
"#,
    );

    assert!(output.status.success(), "{text}");
    assert_eq!(report["status"], "success");
    assert_eq!(report["context_reachability"], "reachable");
    assert!(report.get("ensures_outcomes").is_some());
    assert_eq!(report["cover_results"][0]["clause"], "result == 0");
    assert_eq!(report["cover_results"][0]["label"], "zero");
    assert_eq!(report["cover_results"][0]["status"], "covered");
    assert_eq!(report["cover_results"][0]["witness"]["x"], "0");
    assert_eq!(report["cover_results"][0]["witness"]["result"], "0");

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn bitwise_cover_without_bitvec_mode_is_unknown() {
    let (dir, output, report, text) = verify_source(
        "bitwise_cover",
        r#"
atom bw(x: i64) -> i64
requires: x >= 0 && x < 100;
ensures: result >= 0;
cover "odd": (x & 1) == 1;
body: x;
"#,
    );

    assert!(output.status.success(), "{text}");
    assert_eq!(report["cover_results"][0]["status"], "unknown");
    assert!(
        text.contains("semantics: bitvec")
            || report["diagnostics"].as_array().is_some_and(|diagnostics| {
                diagnostics.iter().any(|diagnostic| {
                    diagnostic
                        .as_str()
                        .is_some_and(|text| text.contains("semantics: bitvec"))
                })
            }),
        "{text}\n{report}"
    );

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn bitwise_cover_with_explicit_bitvec_mode_is_covered() {
    let (dir, output, report, text) = verify_source(
        "bitwise_cover_bitvec",
        r#"
atom bw(x: i64) -> i64
semantics: bitvec;
requires: x >= 0 && x < 100;
ensures: result >= 0;
cover "odd": (x & 1) == 1;
body: x;
"#,
    );

    assert!(output.status.success(), "{text}");
    assert_eq!(report["cover_results"][0]["status"], "covered");
    assert!(is_odd_witness(&report["cover_results"][0]["witness"]["x"]));

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn bitwise_cover_keeps_callers_in_int_mode() {
    let (dir, output, _report, text) = verify_source(
        "bitwise_cover_int_caller",
        r#"
atom bw(x: i64) -> i64
requires: x >= 0 && x < 100;
ensures: result == x;
cover "odd": (x & 1) == 1;
body: x;

atom caller(x: i64) -> i64
requires: x >= 0 && x < 100;
ensures: result == x;
body: bw(x);
"#,
    );

    assert!(output.status.success(), "{text}");

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn call_facts_from_one_cover_do_not_leak_into_later_covers() {
    let (dir, output, report, text) = verify_source(
        "cover_call_facts_leak",
        r#"
atom neg(y: i64) -> i64
requires: y < 0;
ensures: result == y && y < 0;
body: y;

atom caller(x: i64) -> i64
requires: x > -10 && x < 10;
ensures: result == x;
cover "neg call": neg(x) == -1;
cover "positive": x == 5;
body: x;
"#,
    );

    assert!(output.status.success(), "{text}");
    assert_eq!(report["cover_results"][0]["label"], "neg call");
    assert_eq!(report["cover_results"][0]["status"], "covered");
    assert_eq!(report["cover_results"][1]["label"], "positive");
    assert_eq!(report["cover_results"][1]["status"], "covered");
    assert_eq!(report["cover_results"][1]["witness"]["x"], "5");

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn callee_preconditions_constrain_cover_witnesses() {
    let (dir, output, report, text) = verify_source(
        "cover_call",
        r#"
atom pos(y: i64) -> i64
requires: y > 0;
ensures: result == y;
body: y;

atom caller(x: i64) -> i64
requires: x > -10 && x < 10;
ensures: result == x;
cover "pos call": pos(x) == 1;
body: x;
"#,
    );

    assert!(output.status.success(), "{text}");
    assert_eq!(report["cover_results"][0]["status"], "covered");
    assert_eq!(report["cover_results"][0]["witness"]["x"], "1");

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn callee_preconditions_can_make_a_cover_unreachable() {
    let (dir, output, report, text) = verify_source(
        "cover_call_unreachable",
        r#"
atom pos(y: i64) -> i64
requires: y > 0;
ensures: result == y;
body: y;

atom caller(x: i64) -> i64
requires: x < 0;
cover "pos call": pos(x) == 1;
body: x;
"#,
    );

    assert!(!output.status.success(), "{text}");
    assert_eq!(report["failure_type"], "cover_unreachable");

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn check_requires_constrain_cover_witnesses() {
    let (dir, output, report, text) = verify_source(
        "check_requires_negative_cover",
        r#"
atom f(x: i64) -> i64
requires: x > -10 && x < 10;
requires check: x > 0;
ensures: result == x;
cover "neg": x < 0;
body: x;
"#,
    );

    assert!(!output.status.success(), "{text}");
    assert_eq!(report["failure_type"], "cover_unreachable");

    std::fs::remove_dir_all(dir).expect("remove fixture directory");

    let (dir, output, report, text) = verify_source(
        "check_requires_positive_cover",
        r#"
atom sibling(x: i64) -> i64
requires: x > -10 && x < 10;
requires check: x > 0;
ensures: result == x;
cover "pos": x == 5;
body: x;
"#,
    );

    assert!(output.status.success(), "{text}");
    assert_eq!(report["cover_results"][0]["status"], "covered");
    assert_eq!(report["cover_results"][0]["witness"]["x"], "5");

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn guarded_callee_calls_remain_coverable() {
    let (dir, output, report, text) = verify_source(
        "guarded_cover_call",
        r#"
atom pos(y: i64) -> i64
requires: y > 0;
ensures: result == y;
body: y;

atom caller(x: i64) -> i64
requires: x > -10 && x < 10;
ensures: result == x;
cover "pos call": x > 0 && pos(x) == x;
body: x;
"#,
    );

    assert!(output.status.success(), "{text}");
    assert_eq!(report["cover_results"][0]["status"], "covered");

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn tuple_result_cover_is_reported_unknown() {
    let (dir, output, report, text) = verify_source(
        "tuple_cover_result",
        r#"
atom T(x: u64, y: u64) -> (u64, bool)
requires: x + y <= 100;
cover "impossible": result._0 == 999;
body: x + y;
"#,
    );

    assert!(output.status.success(), "{text}");
    assert_eq!(report["cover_results"][0]["status"], "unknown");
    assert!(report["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .any(|diagnostic| diagnostic.as_str().is_some_and(|text| {
            text == "warning: reachability of cover clause \"impossible\" is unknown because tuple result components are not linked to the body"
        })));

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn cover_witness_reports_pre_body_parameter_values() {
    let (dir, output, report, text) = verify_source(
        "mutated_cover_input",
        r#"
atom m(x: i64) -> i64
requires: x >= 0 && x < 100;
ensures: result >= 1;
cover "one": result == 1;
body: { x = x + 1; x };
"#,
    );

    assert!(output.status.success(), "{text}");
    assert_eq!(report["cover_results"][0]["status"], "covered");
    assert_eq!(report["cover_results"][0]["witness"]["x"], "0");
    assert_eq!(report["cover_results"][0]["witness"]["result"], "1");

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn cover_results_survive_verification_cache_hits() {
    let (dir, first_output, first_report, first_text) = verify_source(
        "cached_cover",
        r#"
atom even_cover(x: i64) -> i64
requires: x >= 0 && x < 100;
cover "zero": x == 0;
body: x;
"#,
    );
    assert!(first_output.status.success(), "{first_text}");
    assert_eq!(first_report["cover_results"][0]["status"], "covered");

    let fixture = dir.join("cached_cover.mm");
    let report_dir = dir.join("reports");
    let second_output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--report-dir")
        .arg(&report_dir)
        .arg("--disable-spurious-detection")
        .arg(&fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cached mumei verify");
    let second_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&second_output.stdout),
        String::from_utf8_lossy(&second_output.stderr)
    );
    assert!(second_output.status.success(), "{second_text}");
    assert!(
        second_text.contains("skipped (unchanged, cached)"),
        "{second_text}"
    );

    let cached_report: serde_json::Value = serde_json::from_slice(
        &std::fs::read(report_dir.join("report.json")).expect("read cached report.json"),
    )
    .expect("parse cached report.json");
    assert_eq!(cached_report["cover_results"][0]["status"], "covered");
    assert_eq!(cached_report["cover_results"][0]["label"], "zero");
    assert_eq!(cached_report["cover_results"][0]["witness"]["x"], "0");

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn unreachable_cover_fails_with_structured_failure_type() {
    let (dir, output, report, text) = verify_source(
        "unreachable",
        r#"
atom positive(x: i64) -> i64
requires: x > 0;
cover "neg": x < 0;
body: x;
"#,
    );

    assert!(!output.status.success(), "{text}");
    assert_eq!(report["failure_type"], "cover_unreachable");
    assert!(text.contains("neg"), "{text}");
    assert!(text.contains("unreachable"), "{text}");

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn atoms_without_covers_keep_existing_report_shape() {
    let (dir, output, report, text) = verify_source(
        "no_cover",
        r#"
atom identity(x: i64) -> i64
requires: true;
ensures: result == x;
body: x;
"#,
    );

    assert!(output.status.success(), "{text}");
    assert!(report.get("cover_results").is_none());

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn trusted_atoms_warn_when_covers_are_not_checked() {
    let (dir, output, report, text) = verify_source(
        "trusted",
        r#"
trusted atom fixed() -> i64
requires: true;
cover "one": result == 1;
body: 0;
"#,
    );

    assert!(output.status.success(), "{text}");
    assert!(
        report["diagnostics"]
            .as_array()
            .expect("diagnostics array")
            .iter()
            .any(|diagnostic| {
                diagnostic
                    .as_str()
                    .is_some_and(|text| text.contains("cover clauses") && text.contains("warning"))
            }),
        "{report}"
    );

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn unverified_atoms_do_not_skip_cover_checks() {
    let (dir, output, report, text) = verify_source(
        "unverified_cover",
        r#"
unverified atom u(x: i64) -> i64
cover "never": x != x;
body: x;
"#,
    );

    assert!(!output.status.success(), "{text}");
    assert_eq!(report["failure_type"], "cover_unreachable");

    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

#[test]
fn covers_change_hashes_without_changing_no_cover_golden() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source =
        std::fs::read_to_string(manifest_dir.join("tests/fixtures/bitvec_backward_compat.mm"))
            .expect("read hash compatibility fixture");
    let items = parse_module(&source);
    let atom = items
        .iter()
        .find_map(|item| match item {
            Item::Atom(atom) if atom.name == "bc_add" => Some(atom),
            _ => None,
        })
        .expect("find bc_add");

    let golden: serde_json::Value = serde_json::from_slice(
        &std::fs::read(manifest_dir.join("tests/fixtures/bitvec_backward_compat.golden.json"))
            .expect("read certificate golden"),
    )
    .expect("parse certificate golden");
    let expected_content_hash = golden["atoms"]
        .as_array()
        .expect("golden atoms")
        .iter()
        .find(|entry| entry["name"] == "bc_add")
        .and_then(|entry| entry["content_hash"].as_str())
        .expect("bc_add content hash");
    assert_eq!(compute_atom_content_hash_v2(atom), expected_content_hash);

    let baseline_proof_hash = compute_proof_hash(atom, &ModuleEnv::new());
    let mut covered = atom.clone();
    covered.covers.push(CoverClause {
        clause: "result >= 0".to_string(),
        label: Some("nonnegative".to_string()),
    });
    assert_ne!(
        compute_atom_content_hash_v2(atom),
        compute_atom_content_hash_v2(&covered)
    );
    assert_ne!(
        baseline_proof_hash,
        compute_proof_hash(&covered, &ModuleEnv::new())
    );
}
