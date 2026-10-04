use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const VERIFIED: &str = r#"
atom inc(x: i64) -> i64
requires: true;
ensures: result == x + 1;
body: { x + 1 };
"#;

const FAILED: &str = r#"
atom bad(x: i64) -> i64
requires: true;
ensures: result < x;
body: { x + 1 };
"#;

const VACUOUS: &str = r#"
trusted atom conditional(x: i64) -> i64
requires: true;
ensures: (x != 5 || result == 0) && (x != 5 || result == 1);
body: 0;

atom caller(x: i64) -> i64
requires: x == 5;
ensures: result >= 0;
body: conditional(x);
"#;

const UNKNOWN: &str = r#"
atom fermat3(x: i64, y: i64, z: i64) -> i64
requires: x > 0 && y > 0 && z > 0;
ensures: x * x * x + y * y * y != z * z * z;
body: { 0 };
"#;

const COVER: &str = r#"
atom abs_val(x: i64) -> i64
requires: x > -1000 && x < 1000;
ensures: result >= 0;
cover "zero": result == 0;
body: if x < 0 { 0 - x } else { x };
"#;

fn temp_dir(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "mumei_phase_artifacts_{name}_{}_{}",
        std::process::id(),
        nonce
    ));
    std::fs::create_dir_all(&directory).expect("create temp directory");
    directory
}

fn run_verify(
    input: &Path,
    cwd: &Path,
    report_dir: &Path,
    phase_dir: Option<&Path>,
    json: bool,
    solver_timeout: Option<u64>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .arg("verify")
        .arg(input)
        .arg("--cache-scope")
        .arg("global")
        .arg("--disable-spurious-detection")
        .arg("--report-dir")
        .arg(report_dir);
    if json {
        command.arg("--json");
    }
    if let Some(timeout) = solver_timeout {
        command.arg("--solver-timeout").arg(timeout.to_string());
    }
    if let Some(phase_dir) = phase_dir {
        command.arg("--keep-phase-artifacts").arg(phase_dir);
    }
    command.current_dir(cwd).output().expect("run mumei verify")
}

fn sanitize_path(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

fn find_artifact_component(parent: &Path, value: &str) -> PathBuf {
    let prefix = format!("{}-", sanitize_path(value));
    let matches = std::fs::read_dir(parent)
        .expect("read artifact directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_prefix(&prefix)
                .is_some_and(|hash| {
                    hash.len() == 8
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "expected one artifact component for {value}"
    );
    matches[0].clone()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read JSON artifact"))
        .expect("parse JSON artifact")
}

fn compare_flagged_and_unflagged(
    root: &Path,
    name: &str,
    source: &str,
    solver_timeout: Option<u64>,
) -> PathBuf {
    let input = root.join(format!("{name}.mm"));
    std::fs::write(&input, source).expect("write verification fixture");
    let baseline_cwd = root.join(format!("{name}_baseline"));
    let flagged_cwd = root.join(format!("{name}_flagged"));
    let baseline_reports = baseline_cwd.join("reports");
    let flagged_reports = flagged_cwd.join("reports");
    let phase_dir = flagged_cwd.join("phase_artifacts");
    std::fs::create_dir_all(&baseline_cwd).expect("create baseline cwd");
    std::fs::create_dir_all(&flagged_cwd).expect("create flagged cwd");

    let baseline = run_verify(
        &input,
        &baseline_cwd,
        &baseline_reports,
        None,
        true,
        solver_timeout,
    );
    let flagged = run_verify(
        &input,
        &flagged_cwd,
        &flagged_reports,
        Some(&phase_dir),
        true,
        solver_timeout,
    );
    assert_eq!(baseline.status.code(), flagged.status.code());
    assert_eq!(baseline.stdout, flagged.stdout);
    assert_eq!(
        std::fs::read(baseline_reports.join("report.json")).ok(),
        std::fs::read(flagged_reports.join("report.json")).ok()
    );
    phase_dir
}

#[test]
fn phase_capture_preserves_verification_outputs_and_records_queries() {
    let root = temp_dir("comparison");
    for (name, source, timeout) in [
        ("verified", VERIFIED, None),
        ("failed", FAILED, None),
        ("vacuous", VACUOUS, None),
        ("unknown", UNKNOWN, Some(50)),
        ("cover", COVER, None),
    ] {
        let phase_dir = compare_flagged_and_unflagged(&root, name, source, timeout);
        let atom_name = match name {
            "verified" => "inc",
            "failed" => "bad",
            "vacuous" => "caller",
            "unknown" => "fermat3",
            "cover" => "abs_val",
            _ => unreachable!(),
        };
        let source_path = root.join(format!("{name}.mm"));
        let source_dir =
            find_artifact_component(&phase_dir, source_path.to_string_lossy().as_ref());
        let atom_dir = find_artifact_component(&source_dir, atom_name);
        let phases_json = read_json(&atom_dir.join("phases.json"));
        assert_eq!(phases_json["version"], 1);
        assert_eq!(phases_json["atom"], atom_name);
        assert_eq!(
            phases_json["source_file"],
            source_path.to_string_lossy().as_ref()
        );
        let phases = phases_json["phases"].as_array().expect("phase array");
        let mut previous_index = None;
        for phase in phases {
            let name = phase["phase"].as_str().expect("phase name");
            let index = mumei_core::verification::PHASE_CONTRACTS
                .iter()
                .position(|contract| contract.name == name)
                .unwrap_or_else(|| panic!("phase is not in PHASE_CONTRACTS: {name}"));
            assert!(previous_index.is_none_or(|previous| index >= previous));
            previous_index = Some(index);
            for query in phase["queries"].as_array().expect("phase queries") {
                let query_file = atom_dir.join(query["file"].as_str().unwrap());
                assert!(
                    query_file.exists(),
                    "missing query file {}",
                    query_file.display()
                );
            }
        }
        if name == "failed" {
            assert!(matches!(
                phases.last().unwrap()["result"].as_str(),
                Some("failed" | "aborted" | "unknown")
            ));
        }
        let query_files = std::fs::read_dir(&atom_dir)
            .expect("read atom artifact directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "smt2")
            })
            .collect::<Vec<_>>();
        assert!(!query_files.is_empty(), "no SMT-LIB queries for {name}");
        assert!(query_files.iter().any(|path| {
            std::fs::read_to_string(path).is_ok_and(|query| {
                query.contains("(assert") && query.trim_end().ends_with("(check-sat)")
            })
        }));
    }
    std::fs::remove_dir_all(root).expect("remove fixture directory");
}

#[test]
fn cache_reuse_is_unchanged_and_cached_atoms_are_marked() {
    let root = temp_dir("cache");
    let input = root.join("cached.mm");
    std::fs::write(&input, VERIFIED).expect("write fixture");
    let cwd = root.join("cwd");
    let reports = cwd.join("reports");
    let phase_dir = cwd.join("phase_artifacts");
    std::fs::create_dir_all(&cwd).expect("create current directory");

    let first = run_verify(&input, &cwd, &reports, Some(&phase_dir), false, None);
    assert_eq!(first.status.code(), Some(0));
    let second = run_verify(&input, &cwd, &reports, None, false, None);
    assert_eq!(second.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&second.stdout).contains("skipped (unchanged, cached)"));
    let third = run_verify(&input, &cwd, &reports, Some(&phase_dir), false, None);
    assert_eq!(third.status.code(), Some(0));
    let source_dir = find_artifact_component(&phase_dir, input.to_string_lossy().as_ref());
    let atom_dir = find_artifact_component(&source_dir, "inc");
    let phases_json = read_json(&atom_dir.join("phases.json"));
    assert_eq!(phases_json["outcome"], "cached");
    assert_eq!(phases_json["phases"], serde_json::json!([]));
    std::fs::remove_dir_all(root).expect("remove fixture directory");
}

#[test]
fn artifact_write_failures_warn_without_changing_json_or_exit_code() {
    let root = temp_dir("write_failure");
    let input = root.join("write_failure.mm");
    std::fs::write(&input, VERIFIED).expect("write fixture");

    let discovery_cwd = root.join("discovery");
    let discovery_phase_dir = discovery_cwd.join("phase_artifacts");
    std::fs::create_dir_all(&discovery_cwd).expect("create discovery cwd");
    let discovery = run_verify(
        &input,
        &discovery_cwd,
        &discovery_cwd.join("reports"),
        Some(&discovery_phase_dir),
        false,
        None,
    );
    assert_eq!(discovery.status.code(), Some(0));
    let source_dir =
        find_artifact_component(&discovery_phase_dir, input.to_string_lossy().as_ref());
    let source_component = source_dir
        .file_name()
        .expect("source component")
        .to_os_string();

    let baseline_cwd = root.join("baseline");
    std::fs::create_dir_all(&baseline_cwd).expect("create baseline cwd");
    let baseline = run_verify(
        &input,
        &baseline_cwd,
        &baseline_cwd.join("reports"),
        None,
        true,
        None,
    );

    let blocked_cwd = root.join("blocked");
    let blocked_phase_dir = blocked_cwd.join("phase_artifacts");
    std::fs::create_dir_all(&blocked_phase_dir).expect("create blocked phase directory");
    std::fs::write(blocked_phase_dir.join(source_component), "blocked")
        .expect("block source component");
    let blocked = run_verify(
        &input,
        &blocked_cwd,
        &blocked_cwd.join("reports"),
        Some(&blocked_phase_dir),
        true,
        None,
    );

    assert_eq!(baseline.status.code(), Some(0));
    assert_eq!(baseline.status.code(), blocked.status.code());
    assert_eq!(baseline.stdout, blocked.stdout);
    let warning_lines = String::from_utf8_lossy(&blocked.stderr)
        .lines()
        .filter(|line| line.starts_with("warning: --keep-phase-artifacts: "))
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(warning_lines.len(), 1);
    assert!(warning_lines[0].contains("artifact write(s) failed (first: "));
    assert!(warning_lines[0].ends_with("); phase artifacts are incomplete"));
    std::fs::remove_dir_all(root).expect("remove fixture directory");
}
