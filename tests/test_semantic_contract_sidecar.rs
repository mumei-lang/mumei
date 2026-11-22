use mumei_core::semantic_contract::{
    extract_pure_i64_module_contracts, RefinedAdmission, PURE_I64_COMPOSITE_PROFILE_V24,
    SEMANTIC_CONTRACT_VERSION,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CLAMP_SOURCE: &str = include_str!("fixtures/semantic_contract_clamp.mm");
const INFERRED_SOURCE: &str = include_str!("fixtures/semantic_contract_inferred_i64.mm");

const UNSUPPORTED_EFFECT_SOURCE: &str = r#"
effect FileWrite;

atom write_value(x: i64)
effects: [FileWrite];
requires: x >= 0;
ensures: result >= 0;
body: x;
"#;

const REFUTED_SOURCE: &str = r#"
atom f(x: i64) -> i64
requires: true;
ensures: result == x + 1;
body: x;
"#;

fn run_build(input: &Path, output_base: &Path, semantic_contract: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.args(["build", "--emit", "llvm-ir"]);
    if semantic_contract {
        command.arg("--semantic-contract");
    }
    command
        .arg("-o")
        .arg(output_base)
        .arg(input)
        .current_dir(input.parent().unwrap())
        .output()
        .unwrap()
}

fn run_sidecar_recheck(sidecar: &Path, source: &Path, artifact: &Path, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .args(["evidence", "recheck-sidecar"])
        .arg(sidecar)
        .arg("--source")
        .arg(source)
        .arg("--artifact")
        .arg(artifact);
    if json {
        command.arg("--json");
    }
    command.output().unwrap()
}

fn succeeds(output: Output) -> Output {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn write_input(dir: &Path, name: &str, source: &str) -> PathBuf {
    let input = dir.join(name);
    fs::write(&input, source).unwrap();
    input
}

fn only_llvm_file(dir: &Path) -> PathBuf {
    let paths: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("ll"))
        .collect();
    assert_eq!(
        paths.len(),
        1,
        "expected one LLVM IR file in {}",
        dir.display()
    );
    paths.into_iter().next().unwrap()
}

fn assert_keys(value: &Value, expected: &[&str]) {
    let mut actual: Vec<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    actual.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

fn assert_sidecar_recheck_json(
    output: &Output,
    sidecar_bytes: &[u8],
    artifact_bytes: &[u8],
    contracts: &[mumei_core::semantic_contract::SemanticContract],
) {
    let expected = serde_json::json!({
        "schema_version": "mumei.semantic-contract-sidecar-recheck/v1",
        "sidecar_schema_version": "mumei.semantic-contract-sidecar/v1",
        "sidecar_fingerprint": format!("{:x}", Sha256::digest(sidecar_bytes)),
        "artifact_sha256": format!("{:x}", Sha256::digest(artifact_bytes)),
        "atoms": contracts.iter().map(|contract| contract.atom.clone()).collect::<Vec<_>>(),
        "recheck": "consistent",
        "checks_performed": [
            "sidecar_shape",
            "artifact_hash_binding",
            "contract_fresh_extraction",
            "module_obligations_unsat"
        ],
        "checks_not_performed": [
            "producer_authenticity",
            "source_to_llvm_correspondence",
            "artifact_semantics",
            "parser_encoder_solver_correctness",
            "grade_promotion",
            "lean_kernel_checks"
        ],
        "acceptance": "not_evaluated"
    });
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        expected
    );
}

#[test]
fn llvm_build_writes_hash_bound_semantic_contract_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "clamp.mm", CLAMP_SOURCE);
    let output_base = dir.path().join("clamp");

    succeeds(run_build(&input, &output_base, true));

    let llvm_path = only_llvm_file(dir.path());
    let sidecar_path = llvm_path.with_extension("semantic-contract.json");
    let llvm_bytes = fs::read(&llvm_path).unwrap();
    let sidecar_bytes = fs::read(&sidecar_path).unwrap();
    let sidecar: Value = serde_json::from_slice(&sidecar_bytes).unwrap();
    let expected_contracts = extract_pure_i64_module_contracts(
        CLAMP_SOURCE,
        "clamp.mm",
        None,
        RefinedAdmission::CompositeV24,
    )
    .unwrap();

    assert_keys(
        &sidecar,
        &[
            "schema_version",
            "semantic_contract_version",
            "artifact",
            "artifact_binding",
            "correspondence",
            "contracts",
        ],
    );
    assert_eq!(
        sidecar["schema_version"],
        "mumei.semantic-contract-sidecar/v1"
    );
    assert_eq!(
        sidecar["semantic_contract_version"],
        SEMANTIC_CONTRACT_VERSION
    );
    assert_eq!(
        sidecar["artifact_binding"],
        "sha256_of_emitted_file_not_correspondence"
    );
    assert_eq!(sidecar["correspondence"], "unchecked");
    assert_keys(&sidecar["artifact"], &["kind", "file_name", "sha256"]);
    assert_eq!(sidecar["artifact"]["kind"], "llvm-ir");
    let artifact_file_name = sidecar["artifact"]["file_name"].as_str().unwrap();
    assert_eq!(
        Path::new(artifact_file_name)
            .file_name()
            .and_then(|name| name.to_str()),
        Some(artifact_file_name)
    );
    assert_eq!(
        sidecar["artifact"]["sha256"],
        format!("{:x}", Sha256::digest(&llvm_bytes))
    );
    assert_eq!(
        sidecar["contracts"],
        serde_json::to_value(expected_contracts).unwrap()
    );
    assert!(!String::from_utf8(sidecar_bytes)
        .unwrap()
        .contains(dir.path().to_str().unwrap()));
}

#[test]
fn inferred_i64_sidecar_uses_composite_v24_profile() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "inferred_i64.mm", INFERRED_SOURCE);
    let output_base = dir.path().join("inferred");

    succeeds(run_build(&input, &output_base, true));

    let llvm_path = only_llvm_file(dir.path());
    let sidecar_path = llvm_path.with_extension("semantic-contract.json");
    let sidecar: Value = serde_json::from_slice(&fs::read(sidecar_path).unwrap()).unwrap();
    assert_eq!(
        sidecar["contracts"][0]["profile"],
        PURE_I64_COMPOSITE_PROFILE_V24
    );
}

#[test]
fn unsupported_effect_build_removes_stale_sidecar_and_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "unsupported.mm", UNSUPPORTED_EFFECT_SOURCE);
    let output_base = dir.path().join("unsupported");
    let llvm_path = dir.path().join("unsupported_write_value.ll");
    let sidecar_path = llvm_path.with_extension("semantic-contract.json");
    fs::write(&sidecar_path, "stale sidecar").unwrap();

    let output = run_build(&input, &output_base, true);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("semantic contract sidecar not emitted"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(llvm_path.exists(), "the emitted LLVM IR remains on disk");
    assert!(!sidecar_path.exists(), "stale sidecar must be removed");
}

#[test]
fn semantic_contract_requires_llvm_ir_before_building() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "clamp.mm", CLAMP_SOURCE);
    let output_base = dir.path().join("non_llvm");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["build", "--semantic-contract", "--emit", "proof-cert", "-o"])
        .arg(&output_base)
        .arg(&input)
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--semantic-contract is supported only")
    );
    assert!(!dir.path().join("non_llvm.proof-cert.json").exists());
    assert!(!dir.path().join("non_llvm_clamp_nonnegative.ll").exists());
    assert!(!dir
        .path()
        .join("non_llvm_clamp_nonnegative.semantic-contract.json")
        .exists());
}

#[test]
fn build_without_semantic_contract_does_not_create_or_modify_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "clamp.mm", CLAMP_SOURCE);
    let output_base = dir.path().join("plain");

    succeeds(run_build(&input, &output_base, false));
    let llvm_path = only_llvm_file(dir.path());
    let sidecar_path = llvm_path.with_extension("semantic-contract.json");
    assert!(!sidecar_path.exists());

    fs::write(&sidecar_path, "leave unchanged").unwrap();
    succeeds(run_build(&input, &output_base, false));
    assert_eq!(fs::read(sidecar_path).unwrap(), b"leave unchanged");
}

#[test]
fn recheck_clamp_sidecar_emits_exact_json_and_text_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "clamp.mm", CLAMP_SOURCE);
    let output_base = dir.path().join("clamp");
    succeeds(run_build(&input, &output_base, true));

    let llvm_path = only_llvm_file(dir.path());
    let sidecar_path = llvm_path.with_extension("semantic-contract.json");
    let sidecar_bytes = fs::read(&sidecar_path).unwrap();
    let artifact_bytes = fs::read(&llvm_path).unwrap();
    let contracts = extract_pure_i64_module_contracts(
        CLAMP_SOURCE,
        "clamp.mm",
        None,
        RefinedAdmission::CompositeV24,
    )
    .unwrap();

    let output = succeeds(run_sidecar_recheck(&sidecar_path, &input, &llvm_path, true));
    assert_sidecar_recheck_json(&output, &sidecar_bytes, &artifact_bytes, &contracts);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(dir.path().to_str().unwrap()));

    let output = succeeds(run_sidecar_recheck(
        &sidecar_path,
        &input,
        &llvm_path,
        false,
    ));
    assert_eq!(
        output.stdout,
        b"Semantic-contract sidecar consistent with fresh source extraction, module obligations and artifact hash.\nNo producer authenticity, LLVM correspondence, artifact semantics or acceptance.\n"
    );
}

#[test]
fn recheck_inferred_i64_sidecar_emits_exact_json() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "inferred_i64.mm", INFERRED_SOURCE);
    let output_base = dir.path().join("inferred");
    succeeds(run_build(&input, &output_base, true));

    let llvm_path = only_llvm_file(dir.path());
    let sidecar_path = llvm_path.with_extension("semantic-contract.json");
    let sidecar_bytes = fs::read(&sidecar_path).unwrap();
    let artifact_bytes = fs::read(&llvm_path).unwrap();
    let contracts = extract_pure_i64_module_contracts(
        INFERRED_SOURCE,
        "inferred_i64.mm",
        None,
        RefinedAdmission::CompositeV24,
    )
    .unwrap();

    let output = succeeds(run_sidecar_recheck(&sidecar_path, &input, &llvm_path, true));
    assert_sidecar_recheck_json(&output, &sidecar_bytes, &artifact_bytes, &contracts);
}

#[test]
fn recheck_rejects_artifact_bytes_that_differ_from_hash_binding() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "clamp.mm", CLAMP_SOURCE);
    let output_base = dir.path().join("clamp");
    succeeds(run_build(&input, &output_base, true));

    let llvm_path = only_llvm_file(dir.path());
    let sidecar_path = llvm_path.with_extension("semantic-contract.json");
    let mut artifact_bytes = fs::read(&llvm_path).unwrap();
    artifact_bytes.push(b' ');
    fs::write(&llvm_path, artifact_bytes).unwrap();

    let output = run_sidecar_recheck(&sidecar_path, &input, &llvm_path, true);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("artifact differs from sidecar hash binding"));
}

#[test]
fn recheck_rejects_source_contracts_that_differ_from_fresh_extraction() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "clamp.mm", CLAMP_SOURCE);
    let output_base = dir.path().join("clamp");
    succeeds(run_build(&input, &output_base, true));

    let llvm_path = only_llvm_file(dir.path());
    let sidecar_path = llvm_path.with_extension("semantic-contract.json");
    let changed_source = CLAMP_SOURCE.replace("x >= 0 => result == x", "x >= 0 => result == x + 1");
    assert_ne!(changed_source, CLAMP_SOURCE);
    let changed_input = write_input(dir.path(), "changed.mm", &changed_source);

    let output = run_sidecar_recheck(&sidecar_path, &changed_input, &llvm_path, true);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("sidecar contracts differ from fresh extraction"));
}

#[test]
fn recheck_refutes_a_freshly_extracted_but_false_module_obligation() {
    let dir = tempfile::tempdir().unwrap();
    let source = write_input(dir.path(), "f.mm", REFUTED_SOURCE);
    let artifact = dir.path().join("unrelated-artifact.ll");
    fs::write(&artifact, b"not interpreted as LLVM IR\n").unwrap();
    let artifact_bytes = fs::read(&artifact).unwrap();
    let contracts = extract_pure_i64_module_contracts(
        REFUTED_SOURCE,
        "f.mm",
        None,
        RefinedAdmission::CompositeV24,
    )
    .unwrap();
    let sidecar_value = serde_json::json!({
        "schema_version": "mumei.semantic-contract-sidecar/v1",
        "semantic_contract_version": SEMANTIC_CONTRACT_VERSION,
        "artifact": {
            "kind": "llvm-ir",
            "file_name": "some-other-name.ll",
            "sha256": format!("{:x}", Sha256::digest(&artifact_bytes)),
        },
        "artifact_binding": "sha256_of_emitted_file_not_correspondence",
        "correspondence": "unchecked",
        "contracts": serde_json::to_value(contracts).unwrap(),
    });
    let sidecar = dir.path().join("f.semantic-contract.json");
    fs::write(&sidecar, serde_json::to_vec(&sidecar_value).unwrap()).unwrap();

    let output = run_sidecar_recheck(&sidecar, &source, &artifact, true);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "Semantic-contract sidecar obligation refuted within profile; no recheck diagnostic emitted"
    ));
}

#[test]
fn recheck_rejects_invalid_sidecar_shapes_without_output() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "clamp.mm", CLAMP_SOURCE);
    let output_base = dir.path().join("clamp");
    succeeds(run_build(&input, &output_base, true));

    let llvm_path = only_llvm_file(dir.path());
    let sidecar_path = llvm_path.with_extension("semantic-contract.json");
    let original: Value = serde_json::from_slice(&fs::read(&sidecar_path).unwrap()).unwrap();

    let mut extra_key = original.clone();
    extra_key["unexpected"] = Value::Bool(true);
    let mut wrong_schema = original.clone();
    wrong_schema["schema_version"] = Value::String("other/v1".to_owned());
    let mut mismatched_label = original.clone();
    let mut contracts = mismatched_label["contracts"].as_array().unwrap().clone();
    let mut second_contract = contracts[0].clone();
    second_contract["source_span"]["file"] = Value::String("other.mm".to_owned());
    contracts.push(second_contract);
    mismatched_label["contracts"] = Value::Array(contracts);
    let mut checked_correspondence = original;
    checked_correspondence["correspondence"] = Value::String("checked".to_owned());

    for (name, value) in [
        ("extra-key", extra_key),
        ("wrong-schema", wrong_schema),
        ("mismatched-label", mismatched_label),
        ("checked-correspondence", checked_correspondence),
    ] {
        let sidecar = dir.path().join(format!("{name}.json"));
        fs::write(&sidecar, serde_json::to_vec(&value).unwrap()).unwrap();
        let output = run_sidecar_recheck(&sidecar, &input, &llvm_path, true);
        assert_eq!(output.status.code(), Some(1), "{name}");
        assert!(output.stdout.is_empty(), "{name}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("invalid semantic-contract sidecar"),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn recheck_rejects_sources_outside_the_sidecar_profile() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "clamp.mm", CLAMP_SOURCE);
    let output_base = dir.path().join("clamp");
    succeeds(run_build(&input, &output_base, true));

    let llvm_path = only_llvm_file(dir.path());
    let sidecar_path = llvm_path.with_extension("semantic-contract.json");
    let unsupported_source = write_input(dir.path(), "unsupported.mm", UNSUPPORTED_EFFECT_SOURCE);

    let output = run_sidecar_recheck(&sidecar_path, &unsupported_source, &llvm_path, true);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("source outside semantic-contract sidecar profile"));
}

#[test]
fn recheck_bounds_deeply_nested_source_before_contract_extraction() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "clamp.mm", CLAMP_SOURCE);
    let output_base = dir.path().join("clamp");
    succeeds(run_build(&input, &output_base, true));

    let llvm_path = only_llvm_file(dir.path());
    let sidecar_path = llvm_path.with_extension("semantic-contract.json");
    let nested_source = format!(
        "atom f(x: i64) -> i64 requires: true; ensures: result == {}x{}; body: x;",
        "(".repeat(20_000),
        ")".repeat(20_000)
    );
    let nested_input = write_input(dir.path(), "nested.mm", &nested_source);

    let output = run_sidecar_recheck(&sidecar_path, &nested_input, &llvm_path, true);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("source outside semantic-contract sidecar profile"));
}
