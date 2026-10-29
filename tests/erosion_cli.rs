use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mumei_erosion_cli_{label}_{}_{}",
            std::process::id(),
            nonce
        ));
        std::fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run mumei")
}

fn write_pair(dir: &Path, baseline: &str, candidate: &str) {
    std::fs::write(dir.join("baseline.mm"), baseline).expect("write baseline");
    std::fs::write(dir.join("candidate.mm"), candidate).expect("write candidate");
}

fn run_json(dir: &Path) -> (Output, Value) {
    let output = run(
        dir,
        &[
            "erosion",
            "candidate.mm",
            "--baseline",
            "baseline.mm",
            "--json",
        ],
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "parse erosion report: {error}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output, report)
}

#[test]
fn identical_contract_shortcuts_without_writing_cache_or_report() {
    let dir = TestDir::new("identical");
    write_pair(
        dir.path(),
        "trusted atom f(x: i64) -> i64 \
         requires: unknown_predicate(x); ensures: result > x; body: 0;",
        "atom f(x: i64) -> i64 \
         requires: unknown_predicate ( x ); ensures: result > x; body: x + 1;",
    );

    let (output, report) = run_json(dir.path());

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(report["schema"], "mumei.erosion-report/v1");
    assert_eq!(report["baseline"], "baseline.mm");
    assert_eq!(report["candidate"], "candidate.mm");
    assert_eq!(report["status"], "pass");
    assert_eq!(report["atoms"][0]["status"], "preserved");
    assert_eq!(report["atoms"][0]["detail"], "contract text unchanged");
    assert_eq!(report["totals"]["checked"], 1);
    assert_eq!(report["totals"]["eroded"], 0);
    assert!(!dir.path().join(".mumei_cache").exists());
    assert!(!dir.path().join("erosion_report.json").exists());
}

#[test]
fn dropped_ensures_fails_with_counterexample() {
    let dir = TestDir::new("post-weakened");
    write_pair(
        dir.path(),
        "trusted atom f(x: i64) -> i64 ensures: result >= x + 1; body: 0;",
        "atom f(x: i64) -> i64 ensures: result >= x; body: x;",
    );

    let (output, report) = run_json(dir.path());

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(report["status"], "fail");
    assert_eq!(report["atoms"][0]["status"], "post_weakened");
    assert!(report["atoms"][0]["counterexample"].is_object());
}

#[test]
fn statuses_map_to_the_expected_cli_exit_codes() {
    let cases = [
        (
            "stronger-ensures",
            "trusted atom f(x: i64) -> i64 ensures: result >= x; body: 0;",
            "atom f(x: i64) -> i64 ensures: result > x; body: x + 1;",
            "strengthened",
            0,
        ),
        (
            "relaxed-requires",
            "trusted atom f(x: i64) -> i64 requires: x > 0; body: 0;",
            "atom f(x: i64) -> i64 requires: x >= 0; body: 0;",
            "strengthened",
            0,
        ),
        (
            "tightened-requires",
            "trusted atom f(x: i64) -> i64 requires: x >= 0; body: 0;",
            "atom f(x: i64) -> i64 requires: x > 0; body: 0;",
            "pre_weakened",
            1,
        ),
        (
            "missing-atom",
            "trusted atom f(x: i64) -> i64 body: 0;",
            "",
            "missing",
            1,
        ),
    ];

    for (label, baseline, candidate, expected_status, expected_exit) in cases {
        let dir = TestDir::new(label);
        write_pair(dir.path(), baseline, candidate);
        let (output, report) = run_json(dir.path());

        assert_eq!(output.status.code(), Some(expected_exit), "{label}");
        assert_eq!(report["atoms"][0]["status"], expected_status, "{label}");
    }
}

#[test]
fn trust_escalation_fails_and_trust_removal_passes() {
    let dir = TestDir::new("trust");
    write_pair(
        dir.path(),
        "atom f(x: i64) -> i64 ensures: result == x; body: x;",
        "trusted atom f(x: i64) -> i64 ensures: result == x; body: x;",
    );

    let (trust_added_output, trust_added_report) = run_json(dir.path());
    assert_eq!(trust_added_output.status.code(), Some(1));
    assert_eq!(trust_added_report["atoms"][0]["status"], "trust_added");
    assert_eq!(
        trust_added_report["atoms"][0]["detail"],
        "candidate marks a verified baseline atom as trusted"
    );

    write_pair(
        dir.path(),
        "trusted atom f(x: i64) -> i64 ensures: result == x; body: x;",
        "atom f(x: i64) -> i64 ensures: result == x; body: x;",
    );
    let (trust_removed_output, trust_removed_report) = run_json(dir.path());
    assert_eq!(trust_removed_output.status.code(), Some(0));
    assert_eq!(trust_removed_report["atoms"][0]["status"], "preserved");
}

#[test]
fn effect_widening_fails_while_removal_reordering_and_parameter_match_are_checked() {
    let dir = TestDir::new("effects");
    let cases = [
        (
            "effect File;\neffect Network;\natom f(x: i64) -> i64 effects: [File]; body: x;",
            "effect File;\neffect Network;\natom f(x: i64) -> i64 effects: [File, Network]; body: x;",
            "effects_widened",
            1,
        ),
        (
            "effect File;\neffect Network;\natom f(x: i64) -> i64 effects: [File, Network]; body: x;",
            "effect File;\neffect Network;\natom f(x: i64) -> i64 effects: [File]; body: x;",
            "preserved",
            0,
        ),
        (
            "effect File;\neffect Network;\natom f(x: i64) -> i64 effects: [File, Network]; body: x;",
            "effect File;\neffect Network;\natom f(x: i64) -> i64 effects: [Network, File]; body: x;",
            "preserved",
            0,
        ),
        (
            "effect File(path: i64);\natom f(x: i64, y: i64) -> i64 effects: [File(x)]; body: x;",
            "effect File(path: i64);\natom f(x: i64, y: i64) -> i64 effects: [File(y)]; body: x;",
            "effects_widened",
            1,
        ),
    ];

    for (index, (baseline, candidate, expected_status, expected_exit)) in
        cases.into_iter().enumerate()
    {
        write_pair(dir.path(), baseline, candidate);
        let (output, report) = run_json(dir.path());
        assert_eq!(output.status.code(), Some(expected_exit), "case {index}");
        assert_eq!(
            report["atoms"][0]["status"], expected_status,
            "case {index}"
        );
    }
}

#[test]
fn effect_hierarchy_and_negative_effects_follow_narrowing_semantics() {
    let dir = TestDir::new("effect-hierarchy-negative");
    let hierarchy = "effect FileRead;\neffect FileWrite;\n\
                     effect IO includes: [FileRead, FileWrite];\n";
    let cases = [
        (
            format!("{hierarchy}atom f(x: i64) -> i64 effects: [IO]; body: x;"),
            format!("{hierarchy}atom f(x: i64) -> i64 effects: [FileRead]; body: x;"),
            "preserved",
            0,
        ),
        (
            format!("{hierarchy}atom f(x: i64) -> i64 effects: [FileRead]; body: x;"),
            format!("{hierarchy}atom f(x: i64) -> i64 effects: [IO]; body: x;"),
            "effects_widened",
            1,
        ),
        (
            "effect Network;\natom f(x: i64) -> i64 effects: [!Network]; body: x;".to_string(),
            "effect Network;\natom f(x: i64) -> i64 effects: [!Network, !FileRead]; body: x;"
                .to_string(),
            "preserved",
            0,
        ),
        (
            "effect Network;\natom f(x: i64) -> i64 effects: [!Network]; body: x;".to_string(),
            "effect Network;\natom f(x: i64) -> i64 body: x;".to_string(),
            "effects_widened",
            1,
        ),
        (
            "effect UnknownBase;\neffect UnknownCandidate;\n\
             atom f(x: i64) -> i64 effects: [UnknownBase]; body: x;"
                .to_string(),
            "effect UnknownBase;\neffect UnknownCandidate;\n\
             atom f(x: i64) -> i64 effects: [UnknownCandidate]; body: x;"
                .to_string(),
            "effects_widened",
            1,
        ),
    ];

    for (index, (baseline, candidate, expected_status, expected_exit)) in
        cases.into_iter().enumerate()
    {
        write_pair(dir.path(), &baseline, &candidate);
        let (output, report) = run_json(dir.path());
        assert_eq!(output.status.code(), Some(expected_exit), "case {index}");
        assert_eq!(
            report["atoms"][0]["status"], expected_status,
            "case {index}"
        );
        if index == 3 {
            assert_eq!(
                report["atoms"][0]["detail"],
                "candidate drops a forbidden effect"
            );
        }
    }
}

#[test]
fn effect_contract_changes_are_inconclusive_and_unchanged_contracts_shortcut() {
    let dir = TestDir::new("effect-contracts");
    let effect = "effect File;\n";
    write_pair(
        dir.path(),
        &format!(
            "{effect}atom f(x: i64) -> i64 effects: [File]; \
             effect_pre: {{ File: Open }}; effect_post: {{ File: Closed }}; body: x;"
        ),
        &format!(
            "{effect}atom f(x: i64) -> i64 effects: [File]; \
             effect_pre: {{ File: Open }}; effect_post: {{ File: Closed }}; body: x + 1;"
        ),
    );

    let (unchanged_output, unchanged_report) = run_json(dir.path());
    assert_eq!(unchanged_output.status.code(), Some(0));
    assert_eq!(unchanged_report["atoms"][0]["status"], "preserved");
    assert_eq!(
        unchanged_report["atoms"][0]["detail"],
        "contract text unchanged"
    );

    write_pair(
        dir.path(),
        &format!(
            "{effect}atom f(x: i64) -> i64 effects: [File]; \
             effect_pre: {{ File: Open }}; effect_post: {{ File: Closed }}; body: x;"
        ),
        &format!(
            "{effect}atom f(x: i64) -> i64 effects: [File]; \
             effect_pre: {{ File: Open }}; effect_post: {{ File: Open }}; body: x;"
        ),
    );

    let (changed_output, changed_report) = run_json(dir.path());
    assert_eq!(changed_output.status.code(), Some(1));
    assert_eq!(changed_report["atoms"][0]["status"], "inconclusive");
    assert_eq!(
        changed_report["atoms"][0]["detail"],
        "effect contracts differ: effect_post"
    );
}

#[test]
fn clause_mode_changes_are_inconclusive_in_both_directions() {
    let dir = TestDir::new("clause-modes");
    let cases = [
        (
            "atom f(x: i64) -> i64 requires assume: x > 0; body: x;",
            "atom f(x: i64) -> i64 requires: x > 0; body: x;",
        ),
        (
            "atom f(x: i64) -> i64 requires: x > 0; body: x;",
            "atom f(x: i64) -> i64 requires assume: x > 0; body: x;",
        ),
    ];

    for (baseline, candidate) in cases {
        write_pair(dir.path(), baseline, candidate);
        let (output, report) = run_json(dir.path());
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(report["atoms"][0]["status"], "inconclusive");
        assert_eq!(report["atoms"][0]["detail"], "clause modes differ");
    }
}

#[test]
fn callback_contract_changes_fail_and_unchanged_body_edits_pass() {
    let dir = TestDir::new("callback-contract");
    let callback_contract = "contract(f): requires: x >= 0, ensures: result >= 0;";
    write_pair(
        dir.path(),
        &format!(
            "atom apply(x: i64, f: atom_ref(i64) -> i64) -> i64 \
             requires: x >= 0; ensures: result >= 0; {callback_contract} body: x;"
        ),
        "atom apply(x: i64, f: atom_ref(i64) -> i64) -> i64 \
         requires: x >= 0; ensures: result >= 0; body: x;",
    );

    let (changed_output, changed_report) = run_json(dir.path());
    assert_eq!(changed_output.status.code(), Some(1));
    assert_eq!(changed_report["atoms"][0]["status"], "inconclusive");
    assert_eq!(
        changed_report["atoms"][0]["detail"],
        "callback contract differs: fn_contract_requires"
    );

    write_pair(
        dir.path(),
        &format!(
            "atom apply(x: i64, f: atom_ref(i64) -> i64) -> i64 \
             requires: x >= 0; ensures: result >= 0; {callback_contract} body: x;"
        ),
        &format!(
            "atom apply(x: i64, f: atom_ref(i64) -> i64) -> i64 \
             requires: x >= 0; ensures: result >= 0; {callback_contract} body: x + 1;"
        ),
    );

    let (body_only_output, body_only_report) = run_json(dir.path());
    assert_eq!(body_only_output.status.code(), Some(0));
    assert_eq!(body_only_report["atoms"][0]["status"], "preserved");
    assert_eq!(
        body_only_report["atoms"][0]["detail"],
        "contract text unchanged"
    );
}

#[test]
fn baseline_is_parsed_without_resolving_its_imports() {
    let dir = TestDir::new("baseline-import");
    write_pair(
        dir.path(),
        "import \"missing_baseline_dependency.mm\";\n\
         trusted atom f(x: i64) -> i64 ensures: result >= x; body: 0;",
        "atom f(x: i64) -> i64 ensures: result >= x; body: x;",
    );

    let (output, report) = run_json(dir.path());

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(report["atoms"][0]["status"], "preserved");
}

#[test]
fn generic_candidate_is_loaded_from_source_for_matching() {
    let dir = TestDir::new("generic");
    write_pair(
        dir.path(),
        "trusted atom identity<T>(x: T) -> T ensures: result == x; body: x;",
        "atom identity<T>(x: T) -> T body: x;",
    );

    let (output, report) = run_json(dir.path());

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(report["atoms"][0]["name"], "identity");
    assert_eq!(report["atoms"][0]["status"], "post_weakened");
}

#[test]
fn missing_and_unparsable_inputs_are_input_errors() {
    let dir = TestDir::new("input-errors");
    std::fs::write(
        dir.path().join("baseline.mm"),
        "trusted atom f(x: i64) -> i64 body: 0;",
    )
    .expect("write baseline");
    std::fs::write(
        dir.path().join("candidate.mm"),
        "atom f(x: i64) -> i64 body: 0;",
    )
    .expect("write candidate");

    let missing_candidate = run(
        dir.path(),
        &[
            "erosion",
            "missing.mm",
            "--baseline",
            "baseline.mm",
            "--json",
        ],
    );
    assert_eq!(missing_candidate.status.code(), Some(4));

    let missing_baseline = run(
        dir.path(),
        &[
            "erosion",
            "candidate.mm",
            "--baseline",
            "missing.mm",
            "--json",
        ],
    );
    assert_eq!(missing_baseline.status.code(), Some(4));

    std::fs::write(dir.path().join("baseline.mm"), "this is not valid mumei\n")
        .expect("write invalid baseline");
    let invalid_baseline = run(
        dir.path(),
        &[
            "erosion",
            "candidate.mm",
            "--baseline",
            "baseline.mm",
            "--json",
        ],
    );
    assert_eq!(invalid_baseline.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&invalid_baseline.stderr).contains("erosion input error"));

    std::fs::write(
        dir.path().join("baseline.mm"),
        "trusted atom f(x: i64) -> i64 body: 0;",
    )
    .expect("restore valid baseline");
    std::fs::write(dir.path().join("candidate.mm"), "this is not valid mumei\n")
        .expect("write invalid candidate");
    let invalid_candidate = run(
        dir.path(),
        &[
            "erosion",
            "candidate.mm",
            "--baseline",
            "baseline.mm",
            "--json",
        ],
    );
    assert_eq!(invalid_candidate.status.code(), Some(4));

    std::fs::write(
        dir.path().join("candidate.mm"),
        "import \"missing_candidate_dependency.mm\";\n\
         atom f(x: i64) -> i64 body: 0;",
    )
    .expect("write candidate with unresolved import");
    let candidate_load_failure = run(
        dir.path(),
        &[
            "erosion",
            "candidate.mm",
            "--baseline",
            "baseline.mm",
            "--json",
        ],
    );
    assert_eq!(candidate_load_failure.status.code(), Some(4));
}

#[test]
fn json_output_is_deterministic_and_added_names_are_sorted() {
    let dir = TestDir::new("deterministic");
    write_pair(
        dir.path(),
        "trusted atom f(x: i64) -> i64 body: 0;",
        "atom f(x: i64) -> i64 body: x;\n\
         atom z() -> i64 body: 0;\n\
         atom a() -> i64 body: 0;",
    );

    let first = run(
        dir.path(),
        &[
            "erosion",
            "candidate.mm",
            "--baseline",
            "baseline.mm",
            "--json",
        ],
    );
    let second = run(
        dir.path(),
        &[
            "erosion",
            "candidate.mm",
            "--baseline",
            "baseline.mm",
            "--json",
        ],
    );
    let report: Value = serde_json::from_slice(&first.stdout).expect("parse report");

    assert_eq!(first.status.code(), Some(0));
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(report["added"], serde_json::json!(["a", "z"]));
}

#[test]
fn human_summary_is_short_and_does_not_write_a_report_file() {
    let dir = TestDir::new("human");
    write_pair(
        dir.path(),
        "trusted atom f(x: i64) -> i64 body: 0;",
        "atom f(x: i64) -> i64 body: x;",
    );

    let output = run(
        dir.path(),
        &["erosion", "candidate.mm", "--baseline", "baseline.mm"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(0));
    assert!(stdout.starts_with("Contract erosion pass: 1 checked, 0 eroded, 0 added."));
    assert!(!dir.path().join("erosion_report.json").exists());
}
