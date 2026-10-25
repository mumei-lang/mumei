use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new(source: &str) -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mumei-controls-{id}"));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("input.mm"), source).unwrap();
        Self(path)
    }

    fn input(&self) -> PathBuf {
        self.0.join("input.mm")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(input: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("controls")
        .arg(input)
        .args(args)
        .current_dir(input.parent().expect("input parent directory"))
        .output()
        .unwrap()
}

fn parse(stdout: &[u8]) -> Value {
    serde_json::from_slice(stdout).unwrap_or_else(|error| {
        panic!(
            "invalid controls JSON: {error}\n{}",
            String::from_utf8_lossy(stdout)
        )
    })
}

#[test]
fn strong_contract_is_discriminating_and_weak_contract_fails() {
    let strong = Fixture::new(
        "atom add(x: i64, y: i64) -> i64\nrequires: true;\nensures: result == x + y;\nbody: x + y;\n",
    );
    let output = run(&strong.input(), &["--json"]);
    assert_eq!(output.status.code(), Some(0));
    let report = parse(&output.stdout);
    assert_eq!(report["schema"], "mumei.controls-report/v1");
    assert_eq!(report["atoms"][0]["status"], "discriminating");
    assert_eq!(report["atoms"][0]["tested"], 1);
    assert_eq!(report["atoms"][0]["killed"], 1);
    assert_eq!(report["atoms"][0]["survived"], 0);
    assert_eq!(report["atoms"][0]["unanalyzable"], 0);
    assert_eq!(report["atoms"][0]["mutants"][0]["outcome"], "killed");

    let weak = Fixture::new(
        "atom add(x: i64, y: i64) -> i64\nrequires: x >= 0 && y >= 0;\nensures: result >= 0;\nbody: x + y + 1;\n",
    );
    let output = run(&weak.input(), &["--json"]);
    assert_eq!(output.status.code(), Some(1));
    let report = parse(&output.stdout);
    assert_eq!(report["status"], "fail");
    assert_eq!(report["atoms"][0]["status"], "weak");
    assert!(report["atoms"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("constant:1->0"));
}

#[test]
fn output_is_deterministic_and_cap_is_per_atom() {
    let fixture = Fixture::new(
        "atom add(x: i64, y: i64) -> i64\nrequires: true;\nensures: true;\nbody: x + y + 1;\n",
    );
    let first = run(&fixture.input(), &["--json", "--max-mutants", "1"]);
    let second = run(&fixture.input(), &["--json", "--max-mutants", "1"]);
    assert_eq!(first.stdout, second.stdout);
    assert_eq!(parse(&first.stdout)["atoms"][0]["tested"], 1);
    assert!(!fixture.0.join(".mumei_cache").exists());
}

#[test]
fn trivial_contract_is_weak_and_immutable_body_has_no_mutants() {
    let weak = Fixture::new(
        "atom identity(x: i64) -> i64\nrequires: true;\nensures: true;\nbody: x + 1;\n",
    );
    let output = run(&weak.input(), &["--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(parse(&output.stdout)["atoms"][0]["status"], "weak");

    let immutable = Fixture::new(
        "atom identity(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: x;\n",
    );
    let output = run(&immutable.input(), &["--json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(parse(&output.stdout)["atoms"][0]["status"], "no_mutants");
}

#[test]
fn trusted_atoms_and_original_verification_failures_are_not_checked() {
    let trusted = Fixture::new(
        "trusted atom external(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: 0;\n",
    );
    let output = run(&trusted.input(), &["--json"]);
    assert_eq!(output.status.code(), Some(0));
    let report = parse(&output.stdout);
    assert_eq!(report["atoms"][0]["status"], "not_checked");
    assert_eq!(report["atoms"][0]["detail"], "trusted atom");

    let original_fails =
        Fixture::new("atom bad(x: i64) -> i64\nrequires: true;\nensures: result > 0;\nbody: x;\n");
    let output = run(&original_fails.input(), &["--json"]);
    assert_eq!(output.status.code(), Some(0));
    let report = parse(&output.stdout);
    assert_eq!(report["status"], "pass");
    assert_eq!(report["atoms"][0]["status"], "not_checked");
    assert_eq!(report["atoms"][0]["detail"], "original does not verify");
}

#[test]
fn generic_atoms_are_not_checked() {
    let fixture =
        Fixture::new("atom identity<T>(x: T) -> T\nrequires: true;\nensures: true;\nbody: x;\n");
    let output = run(&fixture.input(), &["--json"]);
    assert_eq!(output.status.code(), Some(0));
    let report = parse(&output.stdout);
    assert_eq!(report["atoms"][0]["name"], "identity");
    assert_eq!(report["atoms"][0]["status"], "not_checked");
}

#[test]
fn method_atoms_use_qualified_names_and_filtering() {
    let fixture = Fixture::new(
        "struct Point { value: i64 }\n\
         impl Point {\n\
             atom get(self: Point) -> i64\n\
             requires: true;\n\
             ensures: result == self.value;\n\
             body: self.value;\n\
         }\n",
    );
    let all = run(&fixture.input(), &["--json"]);
    assert_eq!(all.status.code(), Some(0));
    let report = parse(&all.stdout);
    assert_eq!(report["atoms"][0]["name"], "Point::get");

    let selected = run(&fixture.input(), &["--json", "--atom", "Point::get"]);
    assert_eq!(selected.status.code(), Some(0));
    let report = parse(&selected.stdout);
    assert_eq!(report["atoms"].as_array().unwrap().len(), 1);
    assert_eq!(report["atoms"][0]["name"], "Point::get");
}

#[test]
fn recursive_method_mutants_keep_termination_checks() {
    let fixture = Fixture::new(
        "struct Counter {}\n\
         impl Counter {\n\
             atom down(self: Counter, n: i64) -> i64\n\
             requires: n >= 0;\n\
             ensures: true;\n\
             decreases: n;\n\
             body: if n == 0 { 0 } else { Counter::down(self, n - 1) };\n\
         }\n",
    );
    let output = run(&fixture.input(), &["--json", "--atom", "Counter::down"]);
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = parse(&output.stdout);
    assert_eq!(report["atoms"][0]["name"], "Counter::down");
    let termination_mutant = report["atoms"][0]["mutants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|mutant| mutant["id"].as_str().unwrap().contains("Sub->Add"))
        .expect("recursive argument subtraction mutant");
    assert_eq!(
        termination_mutant["outcome"], "killed",
        "{}",
        report["atoms"][0]
    );
}

#[test]
fn monomorphized_generic_instances_are_reported_and_selectable() {
    let fixture = Fixture::new(
        "atom id<T>(x: T) -> T\n\
         requires: true;\n\
         ensures: result == x + 1;\n\
         body: x + 1;\n\
         atom main() -> i64\n\
         requires: true;\n\
         ensures: result == 4;\n\
         body: id<i64>(3);\n",
    );
    let all = run(&fixture.input(), &["--json"]);
    assert!(
        matches!(all.status.code(), Some(0 | 1)),
        "{}\n{}",
        String::from_utf8_lossy(&all.stdout),
        String::from_utf8_lossy(&all.stderr)
    );
    let report = parse(&all.stdout);
    let template = report["atoms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|atom| atom["name"] == "id")
        .expect("generic template");
    assert_eq!(template["status"], "not_checked");
    let instance = report["atoms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|atom| atom["name"] == "id<i64>")
        .expect("monomorphized instance");
    assert!(instance["tested"].as_u64().unwrap() > 0);
    assert!(instance["killed"].as_u64().unwrap() > 0);

    let selected = run(&fixture.input(), &["--json", "--atom", "id<i64>"]);
    assert!(matches!(selected.status.code(), Some(0 | 1)));
    let report = parse(&selected.stdout);
    assert_eq!(report["atoms"].as_array().unwrap().len(), 1);
    assert_eq!(report["atoms"][0]["name"], "id<i64>");
}

#[test]
fn condition_flip_mutant_is_rejected_by_a_strong_spec() {
    let fixture = Fixture::new(
        "atom nonnegative(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result >= 0;\n\
         body: if x >= 0 { x } else { 0 };\n",
    );
    let output = run(&fixture.input(), &["--json", "--max-mutants", "1"]);
    assert_eq!(output.status.code(), Some(0));
    let report = parse(&output.stdout);
    let mutants = report["atoms"][0]["mutants"].as_array().unwrap();
    let condition_flip = mutants
        .iter()
        .find(|mutant| mutant["operator"] == "condition-flip")
        .expect("condition-flip mutant");
    assert_eq!(condition_flip["outcome"], "killed");
}

#[test]
fn duplicate_comparison_mutants_do_not_consume_the_cap() {
    let fixture = Fixture::new(
        "atom choose(x: i64, y: i64) -> i64\n\
         requires: x > y;\n\
         ensures: result == x + y;\n\
         body: if x > y { x + y } else { x + y };\n",
    );
    let output = run(&fixture.input(), &["--json", "--max-mutants", "2"]);
    assert!(matches!(output.status.code(), Some(0 | 1)));
    let report = parse(&output.stdout);
    let mutants = report["atoms"][0]["mutants"].as_array().unwrap();
    assert_eq!(mutants.len(), 2);
    assert_ne!(mutants[0]["id"], mutants[1]["id"]);
    assert!(mutants[1]["site"].as_u64().unwrap() > mutants[0]["site"].as_u64().unwrap());
}

#[test]
fn only_the_unconstrained_plus_site_survives() {
    let fixture = Fixture::new(
        "atom choose(x: i64, y: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result == x + 1;\n\
         body: if false { x + y } else { x + 1 };\n",
    );
    let output = run(&fixture.input(), &["--json", "--max-mutants", "2"]);
    assert_eq!(output.status.code(), Some(1));
    let report = parse(&output.stdout);
    let mut outcomes = report["atoms"][0]["mutants"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|mutant| mutant["operator"] == "binary-op-flip")
        .map(|mutant| {
            (
                mutant["site"].as_u64().unwrap(),
                mutant["outcome"].as_str().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    outcomes.sort_by_key(|(site, _)| *site);
    assert_eq!(outcomes, vec![(0, "survived"), (1, "killed")]);
}

#[test]
fn human_output_includes_atom_and_totals_lines() {
    let fixture = Fixture::new(
        "atom identity(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: x;\n",
    );
    let output = run(&fixture.input(), &[]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("identity: no_mutants"));
    assert!(stdout.contains("Totals: "));
}

#[test]
fn input_and_atom_selection_errors_use_exit_four() {
    let fixture =
        Fixture::new("atom id(x: i64) -> i64\nrequires: true;\nensures: result == x;\nbody: x;\n");
    assert_eq!(
        run(&fixture.0.join("missing.mm"), &[]).status.code(),
        Some(4)
    );
    assert_eq!(
        run(&fixture.input(), &["--atom", "missing"]).status.code(),
        Some(4)
    );
    let zero_cap = run(&fixture.input(), &["--max-mutants", "0"]);
    assert_eq!(zero_cap.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&zero_cap.stderr).contains("must be greater than 0"));
    let bad = Fixture::new("atom broken(");
    assert_eq!(run(&bad.input(), &[]).status.code(), Some(4));
}

#[test]
fn verify_json_does_not_expose_internal_verdict_metadata() {
    let fixture =
        Fixture::new("atom bad(x: i64) -> i64\nrequires: true;\nensures: result > 0;\nbody: x;\n");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(fixture.input())
        .arg("--json")
        .output()
        .unwrap();
    let payload = parse(&output.stdout);
    assert_eq!(payload["status"], "failed");
    assert!(payload.get("diagnostics").is_some());
    assert!(payload.to_string().find("\"verdict\"").is_none());
}
