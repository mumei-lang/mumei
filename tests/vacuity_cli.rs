use serde_json::Value;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new(source: &str) -> Self {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("mumei-vacuity-cli-{}-{id}", std::process::id()));
        fs::create_dir_all(&directory).expect("create fixture directory");
        fs::write(directory.join("input.mm"), source).expect("write fixture");
        Self(directory)
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

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn run(input: &Path, extra: &[OsString]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(input)
        .args(extra)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify")
}

fn json_output(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let start = stdout
        .find('{')
        .unwrap_or_else(|| panic!("verify emitted no JSON:\n{stdout}"));
    serde_json::from_str(&stdout[start..])
        .unwrap_or_else(|error| panic!("invalid verify JSON: {error}\n{stdout}"))
}

fn remove_process_timestamps(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(remove_process_timestamps),
        Value::Object(fields) => {
            fields.remove("process_end_time");
            fields.remove("process_start_time");
            fields.values_mut().for_each(remove_process_timestamps);
        }
        _ => {}
    }
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn append_path_args(args: &mut Vec<OsString>, flag: &str, path: &Path) {
    args.push(flag.into());
    args.push(path.as_os_str().to_owned());
}

fn phase_artifact(root: &Path, atom: &str) -> Value {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).expect("read phase artifact directory") {
            let path = entry.expect("phase artifact entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|name| name == "phases.json") {
                let value: Value =
                    serde_json::from_slice(&fs::read(path).expect("read phase artifact"))
                        .expect("valid phase artifact JSON");
                if value["atom"] == atom {
                    return value;
                }
            }
        }
    }
    panic!("no phase artifact for atom {atom} under {}", root.display());
}

#[test]
fn strong_spec_reports_kills_and_no_survivors() {
    let fixture = Fixture::new(
        "atom add(x: i64, y: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x + y;\n\
         body: x + y;\n",
    );
    let output = run(
        &fixture.input(),
        &args(&["--enable-vacuity-check", "--json"]),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("killed 1"), "{stderr}");
    assert!(stderr.contains("survived 0"), "{stderr}");
}

#[test]
fn weak_spec_reports_the_surviving_constant_mutant() {
    let fixture = Fixture::new(
        "atom add(x: i64, y: i64) -> i64\n\
         requires: x >= 0 && y >= 0;\n\
         ensures: result >= 0;\n\
         body: x + y + 1;\n",
    );
    let output = run(
        &fixture.input(),
        &args(&["--enable-vacuity-check", "--json"]),
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined_output(&output).contains("constant:1->0@2"),
        "{output:?}"
    );
}

#[test]
fn trivial_ensures_fails_for_a_mutable_body() {
    let fixture = Fixture::new(
        "atom weak(x: i64) -> i64\n\
         requires: true;\n\
         ensures: true;\n\
         body: x + 1;\n",
    );
    let output = run(
        &fixture.input(),
        &args(&["--enable-vacuity-check", "--json"]),
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        combined_output(&output).contains("surviving mutants ["),
        "{output:?}"
    );
}

#[test]
fn only_the_unconstrained_plus_site_is_reported_as_surviving() {
    let fixture = Fixture::new(
        "atom choose(x: i64, y: i64) -> i64\n\
         requires: y == 0;\n\
         ensures: result == x + x;\n\
         body: x + y + x;\n",
    );
    let output = run(
        &fixture.input(),
        &args(&["--enable-vacuity-check", "--json"]),
    );
    assert_eq!(output.status.code(), Some(1));
    let output_text = combined_output(&output);
    let surviving_ids = output_text
        .split("surviving mutants [")
        .nth(1)
        .and_then(|value| value.split(']').next())
        .expect("surviving mutant ids");
    assert_eq!(surviving_ids, "op-flip:Add->Sub@1", "{output_text}");
}

#[test]
fn condition_flip_is_killed_by_a_strong_spec() {
    let fixture = Fixture::new(
        "atom nonnegative(x: i64, y: i64) -> i64\n\
         requires: x >= y;\n\
         ensures: result == x;\n\
         body: if x >= y { x } else { y };\n",
    );
    let output = run(
        &fixture.input(),
        &args(&["--enable-vacuity-check", "--json"]),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("killed "), "{stderr}");
    assert!(stderr.contains("survived 0"), "{stderr}");
}

#[test]
fn pow_only_and_immutable_bodies_have_no_mutants() {
    let pow = Fixture::new(
        "atom power() -> i64\n\
         requires: true;\n\
         ensures: result == 8;\n\
         body: 2 ** 3;\n",
    );
    let output = run(&pow.input(), &args(&["--enable-vacuity-check", "--json"]));
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!combined_output(&output).contains("Pow->Pow"));

    let immutable = Fixture::new(
        "atom identity(x: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x;\n\
         body: x;\n",
    );
    let output = run(
        &immutable.input(),
        &args(&["--enable-vacuity-check", "--json"]),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined_output(&output).contains("tested 0"));
}

#[test]
fn recursive_method_keeps_its_termination_obligations() {
    let fixture = Fixture::new(
        "struct Counter {}\n\
         impl Counter {\n\
           atom down(self: Counter, n: i64) -> i64\n\
           requires: n >= 0;\n\
           ensures: result == n;\n\
           decreases: n;\n\
           body: if n == 0 { 0 } else { Counter::down(self, n - 1) + 1 };\n\
         }\n",
    );
    let output = run(
        &fixture.input(),
        &args(&["--enable-vacuity-check", "--json"]),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("killed "), "{stderr}");
    assert!(stderr.contains("survived 0"), "{stderr}");
    assert!(stderr.contains("unanalyzable 0"), "{stderr}");
    let summary = stderr
        .lines()
        .find(|line| line.contains("Vacuity check passed for") && line.contains("Counter::down"))
        .expect("vacuity summary");
    let mut counts = summary
        .split("tested ")
        .nth(1)
        .expect("tested count")
        .split(", ");
    let tested = counts
        .next()
        .expect("tested value")
        .parse::<usize>()
        .expect("numeric tested count");
    let killed = counts
        .next()
        .and_then(|value| value.strip_prefix("killed "))
        .expect("killed count")
        .parse::<usize>()
        .expect("numeric killed count");
    assert_eq!(killed, tested, "{summary}");
}

#[test]
fn nested_verification_preserves_report_certificate_json_and_outer_artifacts() {
    let fixture = Fixture::new(
        "atom add(x: i64, y: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x + y;\n\
         body: x + y;\n",
    );
    let report_dir = fixture.0.join("reports");
    let artifacts_dir = fixture.0.join("artifacts");
    let certificate = fixture.0.join("output.proof.json");
    let mut common = args(&["--json", "--proof-cert"]);
    append_path_args(&mut common, "--report-dir", &report_dir);
    append_path_args(&mut common, "--keep-phase-artifacts", &artifacts_dir);
    append_path_args(&mut common, "--output", &certificate);

    let without_flag = run(&fixture.input(), &common);
    assert_eq!(without_flag.status.code(), Some(0));
    let mut report_without_flag = json_output(&without_flag);
    remove_process_timestamps(&mut report_without_flag);
    let mut disk_report_without_flag: Value = serde_json::from_slice(
        &fs::read(report_dir.join("report.json")).expect("read outer report"),
    )
    .expect("valid report JSON");
    remove_process_timestamps(&mut disk_report_without_flag);
    let certificate_without_flag: Value =
        serde_json::from_slice(&fs::read(&certificate).expect("read certificate"))
            .expect("valid certificate JSON");
    let phase_without_flag = phase_artifact(&artifacts_dir, "add");

    let mut with_flag = common.clone();
    with_flag.push("--enable-vacuity-check".into());
    let with_flag = run(&fixture.input(), &with_flag);
    assert_eq!(with_flag.status.code(), Some(0));
    let mut report_with_flag = json_output(&with_flag);
    remove_process_timestamps(&mut report_with_flag);
    assert_eq!(report_with_flag, report_without_flag);
    let mut disk_report_with_flag: Value = serde_json::from_slice(
        &fs::read(report_dir.join("report.json")).expect("read outer report"),
    )
    .expect("valid report JSON");
    remove_process_timestamps(&mut disk_report_with_flag);
    let mut certificate_with_flag: Value =
        serde_json::from_slice(&fs::read(&certificate).expect("read certificate"))
            .expect("valid certificate JSON");
    remove_process_timestamps(&mut certificate_with_flag);
    let mut certificate_without_flag = certificate_without_flag;
    remove_process_timestamps(&mut certificate_without_flag);
    assert_eq!(disk_report_with_flag, disk_report_without_flag);
    assert_eq!(
        certificate_with_flag["atoms"][0],
        certificate_without_flag["atoms"][0]
    );
    assert_eq!(phase_artifact(&artifacts_dir, "add"), phase_without_flag);
}

#[test]
fn nested_verification_does_not_pollute_the_next_atom() {
    let fixture = Fixture::new(
        "atom first(x: i64, y: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x + y;\n\
         body: x + y;\n\
         atom second(x: i64, y: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x + y;\n\
         body: x + y;\n",
    );
    let report_dir = fixture.0.join("reports");
    let artifacts_dir = fixture.0.join("artifacts");
    let mut common = args(&["--json"]);
    append_path_args(&mut common, "--report-dir", &report_dir);
    append_path_args(&mut common, "--keep-phase-artifacts", &artifacts_dir);

    let without_flag = run(&fixture.input(), &common);
    assert_eq!(without_flag.status.code(), Some(0));
    let report_without_flag =
        fs::read(report_dir.join("report.json")).expect("read report without vacuity");
    let second_phase_without_flag = phase_artifact(&artifacts_dir, "second");

    let mut with_flag = common;
    with_flag.push("--enable-vacuity-check".into());
    let with_flag = run(&fixture.input(), &with_flag);
    assert_eq!(with_flag.status.code(), Some(0));
    let report_with_flag =
        fs::read(report_dir.join("report.json")).expect("read report with vacuity");
    assert_eq!(report_with_flag, report_without_flag);
    assert_eq!(
        phase_artifact(&artifacts_dir, "second"),
        second_phase_without_flag
    );
}
