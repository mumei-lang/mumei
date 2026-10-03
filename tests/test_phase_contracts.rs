use mumei_core::verification::phase_contract::{phase_contract, PHASE_CONTRACTS};
use std::path::PathBuf;
use std::process::{Command, Output};

fn write_fixture(name: &str, source: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "mumei_phase_contracts_{}_{}_{}",
        std::process::id(),
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create fixture directory");
    let fixture = dir.join(format!("{name}.mm"));
    std::fs::write(&fixture, source).expect("write fixture");
    (dir, fixture)
}

fn verify(name: &str, source: &str) -> (PathBuf, Output) {
    let (dir, fixture) = write_fixture(name, source);
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(&fixture)
        .arg("--report-dir")
        .arg(&dir)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify");
    (dir, output)
}

fn parse_phase_blocks(stderr: &str) -> Vec<Vec<String>> {
    let mut blocks = Vec::new();
    for line in stderr.lines() {
        if line.contains("[metrics] atom '") && line.ends_with("verification phases:") {
            blocks.push(Vec::new());
            continue;
        }

        let Some(block) = blocks.last_mut() else {
            continue;
        };
        let Some((name, elapsed)) = line.trim().rsplit_once(": ") else {
            continue;
        };
        let Some(milliseconds) = elapsed.strip_suffix("ms") else {
            continue;
        };
        if name.starts_with("Phase ") && milliseconds.parse::<f64>().is_ok() {
            block.push(name.to_string());
        }
    }
    blocks
}

fn assert_phase_blocks_follow_contracts(blocks: &[Vec<String>]) {
    assert!(!blocks.is_empty(), "no phase metrics were recorded");

    for block in blocks {
        let indices: Vec<usize> = block
            .iter()
            .map(|recorded_name| {
                let contract = phase_contract(recorded_name)
                    .unwrap_or_else(|| panic!("unknown recorded phase: {recorded_name}"));
                PHASE_CONTRACTS
                    .iter()
                    .position(|candidate| candidate.name == contract.name)
                    .expect("phase contract belongs to the declared table")
            })
            .collect();
        assert!(
            indices.windows(2).all(|pair| pair[0] <= pair[1]),
            "phase order decreased: {block:?}"
        );
    }
}

#[test]
fn verified_and_failed_atoms_record_phases_in_contract_order() {
    let (verified_dir, verified) = verify(
        "verified",
        r#"
atom inc(x: i64) -> i64
requires: x < 100;
ensures: result > x;
body: { x + 1 };
"#,
    );
    let verified_stderr = String::from_utf8_lossy(&verified.stderr);
    let verified_blocks = parse_phase_blocks(&verified_stderr);

    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    assert_phase_blocks_follow_contracts(&verified_blocks);
    let verified_phases = verified_blocks
        .iter()
        .flatten()
        .map(|name| phase_contract(name).expect("phase is declared").name)
        .collect::<Vec<_>>();
    assert!(verified_phases.contains(&"Phase 4: body evaluation"));
    assert!(verified_phases.contains(&"Phase 5: ensures verification"));
    assert!(verified_phases.contains(&"Phase 6: final Z3 check"));

    let (failure_dir, failure) = verify(
        "failure",
        r#"
atom bad_inc(x: i64) -> i64
requires: true;
ensures: result > x;
body: x;
"#,
    );
    let failure_stderr = String::from_utf8_lossy(&failure.stderr);
    let failure_blocks = parse_phase_blocks(&failure_stderr);

    assert!(!failure.status.success());
    assert_phase_blocks_follow_contracts(&failure_blocks);

    std::fs::remove_dir_all(verified_dir).expect("remove verified fixture directory");
    std::fs::remove_dir_all(failure_dir).expect("remove failure fixture directory");
}
