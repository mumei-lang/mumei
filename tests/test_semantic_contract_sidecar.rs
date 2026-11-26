use mumei_core::semantic_contract::{
    extract_pure_i64_module_contracts, RefinedAdmission, PURE_I64_COMPOSITE_PROFILE_V24,
    PURE_I64_COMPOSITE_PROFILE_V25, PURE_I64_COMPOSITE_PROFILE_V26, PURE_I64_COMPOSITE_PROFILE_V27,
    SEMANTIC_CONTRACT_VERSION,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CLAMP_SOURCE: &str = include_str!("fixtures/semantic_contract_clamp.mm");
const INFERRED_SOURCE: &str = include_str!("fixtures/semantic_contract_inferred_i64.mm");
const PRODUCER_KEY_ID: &str = "producer-a";
const SIGNED_AT: &str = "2024-02-01T00:00:00Z";
const REFERENCE_TIME: &str = "2024-02-02T00:00:00Z";

const UNSUPPORTED_EFFECT_SOURCE: &str = r#"
effect FileWrite(path: i64);

atom write_value(x: i64)
effects: [FileWrite];
requires: x >= 0;
ensures: result >= 0;
body: x;
"#;

const STATEFUL_EFFECT_SOURCE: &str = r#"
effect Vault
    states: [Idle, Checked];
    initial: Idle;
    transition check: Idle -> Checked;

atom main() -> i64
    effects: [Vault];
    effect_pre: { Vault: Idle };
    effect_post: { Vault: Checked };
    requires: true;
    ensures: result == 7;
    body: {
        perform Vault.check;
        7
    }
"#;

const REFUTED_SOURCE: &str = r#"
atom f(x: i64) -> i64
requires: true;
ensures: result == x + 1;
body: x;
"#;

const NON_EXITING_REQUIRES_LOOP_SOURCE: &str = r#"
atom main() -> i64
requires: { let j = 0; while true invariant: j >= 0 decreases: 4 { j = j + 1 }; true };
ensures: result == 0;
body: 0;
"#;

const BOUNDED_LOOP_SOURCE: &str = r#"
atom count(n: i64) -> i64
requires: n >= 0 && n <= 2;
ensures: result == n;
body: { let i = 0; while i < n invariant: i >= 0 && i <= n decreases: n - i { i = i + 1 }; i };

atom main() -> i64
requires: true;
ensures: result == 2;
body: { count(2) };
"#;

const REQUIRES_EMPTY_BOUNDED_LOOP_SOURCE: &str = r#"
atom count(n: i64) -> i64
requires: n >= 10 && n <= 20;
ensures: result == 999;
body: { let i = 0; while i < n invariant: i >= 0 && i <= n decreases: n - i { i = i + 1 }; i };
"#;

const ARRAY_PARAM_SOURCE: &str = r#"
atom at(a: [i64], i: i64) -> i64
requires: i >= 0 && i < len(a);
ensures: result == a[i];
body: a[i];
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

fn sidecar_accept_command(sidecar: &Path, source: &Path, artifact: &Path, json: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .args(["evidence", "accept-sidecar"])
        .arg(sidecar)
        .args(["--source"])
        .arg(source)
        .args(["--artifact"])
        .arg(artifact);
    if json {
        command.arg("--json");
    }
    command
}

fn generate_producer_key(dir: &Path) -> (PathBuf, String) {
    let secret_file = dir.join("producer.secret");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "keygen", "--secret-file"])
        .arg(&secret_file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let public_key = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("Public key: "))
        .unwrap()
        .to_owned();
    (secret_file, public_key)
}

fn sign_file(subject: &Path, secret_file: &Path, produced_at: Option<&str>) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .args(["evidence", "sign"])
        .arg(subject)
        .args(["--key-file"])
        .arg(secret_file)
        .args(["--producer-key-id", PRODUCER_KEY_ID]);
    if let Some(produced_at) = produced_at {
        command.args(["--produced-at", produced_at]);
    }
    let output = command.args(["--json"]).output().unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn write_signature(path: &Path, subject: &Path, secret_file: &Path, produced_at: Option<&str>) {
    let signature = sign_file(subject, secret_file, produced_at);
    fs::write(path, serde_json::to_vec_pretty(&signature).unwrap()).unwrap();
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

fn write_sidecar_fixture(
    dir: &Path,
    source_name: &str,
    source: &str,
    artifact_name: &str,
) -> (PathBuf, PathBuf, PathBuf) {
    write_sidecar_fixture_with_admission(
        dir,
        source_name,
        source,
        artifact_name,
        RefinedAdmission::CompositeV26,
    )
}

fn write_sidecar_fixture_with_admission(
    dir: &Path,
    source_name: &str,
    source: &str,
    artifact_name: &str,
    admission: RefinedAdmission,
) -> (PathBuf, PathBuf, PathBuf) {
    let source_path = write_input(dir, source_name, source);
    let artifact_path = dir.join(artifact_name);
    let artifact_bytes = b"not interpreted as LLVM IR\n";
    fs::write(&artifact_path, artifact_bytes).unwrap();
    let contracts =
        extract_pure_i64_module_contracts(source, source_name, None, admission).unwrap();
    let sidecar_value = serde_json::json!({
        "schema_version": "mumei.semantic-contract-sidecar/v1",
        "semantic_contract_version": SEMANTIC_CONTRACT_VERSION,
        "artifact": {
            "kind": "llvm-ir",
            "file_name": artifact_name,
            "sha256": format!("{:x}", Sha256::digest(artifact_bytes)),
        },
        "artifact_binding": "sha256_of_emitted_file_not_correspondence",
        "correspondence": "unchecked",
        "contracts": serde_json::to_value(contracts).unwrap(),
    });
    let sidecar_path = dir.join("sidecar.semantic-contract.json");
    fs::write(&sidecar_path, serde_json::to_vec(&sidecar_value).unwrap()).unwrap();
    (source_path, artifact_path, sidecar_path)
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

fn build_clamp_sidecar(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let input = write_input(dir, "clamp.mm", CLAMP_SOURCE);
    let output_base = dir.join("clamp");
    succeeds(run_build(&input, &output_base, true));
    let artifact = only_llvm_file(dir);
    let sidecar = artifact.with_extension("semantic-contract.json");
    (input, artifact, sidecar)
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
    let contracts_v24 = extract_pure_i64_module_contracts(
        CLAMP_SOURCE,
        "clamp.mm",
        None,
        RefinedAdmission::CompositeV24,
    )
    .unwrap();
    let contracts_v25 = extract_pure_i64_module_contracts(
        CLAMP_SOURCE,
        "clamp.mm",
        None,
        RefinedAdmission::CompositeV25,
    )
    .unwrap();
    let contracts_v26 = extract_pure_i64_module_contracts(
        CLAMP_SOURCE,
        "clamp.mm",
        None,
        RefinedAdmission::CompositeV26,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_vec(&contracts_v24).unwrap(),
        serde_json::to_vec(&contracts_v25).unwrap()
    );
    assert_eq!(
        serde_json::to_vec(&contracts_v24).unwrap(),
        serde_json::to_vec(&contracts_v26).unwrap()
    );

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
    let develop_sidecar_bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "schema_version": "mumei.semantic-contract-sidecar/v1",
        "semantic_contract_version": SEMANTIC_CONTRACT_VERSION,
        "artifact": {
            "kind": "llvm-ir",
            "file_name": artifact_file_name,
            "sha256": format!("{:x}", Sha256::digest(&llvm_bytes)),
        },
        "artifact_binding": "sha256_of_emitted_file_not_correspondence",
        "correspondence": "unchecked",
        "contracts": serde_json::to_value(contracts_v24).unwrap(),
    }))
    .unwrap();
    assert_eq!(sidecar_bytes, develop_sidecar_bytes);
    assert_eq!(
        sidecar["artifact"]["sha256"],
        format!("{:x}", Sha256::digest(&llvm_bytes))
    );
    assert_eq!(
        sidecar["contracts"],
        serde_json::to_value(contracts_v25).unwrap()
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
fn effect_sidecar_uses_composite_v25_and_rechecks_and_accepts() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "effectful.mm", STATEFUL_EFFECT_SOURCE);
    let output_base = dir.path().join("effectful");
    succeeds(run_build(&input, &output_base, true));

    let artifact = only_llvm_file(dir.path());
    let sidecar = artifact.with_extension("semantic-contract.json");
    let sidecar_value: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!(
        sidecar_value["contracts"][0]["profile"],
        PURE_I64_COMPOSITE_PROFILE_V25
    );
    assert_eq!(
        sidecar_value["contracts"][0]["specification"]["effects"],
        serde_json::json!(["Vault"])
    );
    assert_eq!(
        sidecar_value["contracts"][0]["specification"]["effect_pre"]["Vault"],
        "Idle"
    );
    assert_eq!(
        sidecar_value["contracts"][0]["specification"]["effect_post"]["Vault"],
        "Checked"
    );

    let recheck = succeeds(run_sidecar_recheck(&sidecar, &input, &artifact, true));
    assert_eq!(
        serde_json::from_slice::<Value>(&recheck.stdout).unwrap()["recheck"],
        "consistent"
    );
    let unsigned_accept = sidecar_accept_command(&sidecar, &input, &artifact, true)
        .output()
        .unwrap();
    assert!(
        unsigned_accept.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&unsigned_accept.stdout),
        String::from_utf8_lossy(&unsigned_accept.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&unsigned_accept.stdout).unwrap()["accepted"],
        true
    );

    let (secret, public) = generate_producer_key(dir.path());
    let signature = dir.path().join("effectful.signature.json");
    write_signature(&signature, &sidecar, &secret, None);
    let mut command = sidecar_accept_command(&sidecar, &input, &artifact, true);
    command
        .args(["--signature"])
        .arg(&signature)
        .args(["--producer-key", &public])
        .args(["--expected-producer-key", PRODUCER_KEY_ID]);
    let signed_accept = command.output().unwrap();
    assert!(
        signed_accept.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&signed_accept.stdout),
        String::from_utf8_lossy(&signed_accept.stderr)
    );
    let descriptor: Value = serde_json::from_slice(&signed_accept.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert!(descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .contains(&Value::String("producer_signature_binding".to_owned())));
}

#[test]
fn callback_parameter_sidecar_build_rechecks_and_accepts_v27() {
    let dir = tempfile::tempdir().unwrap();
    let source_text = include_str!("positive/test_callback_contract_ok.mm");
    let input = write_input(dir.path(), "test_callback_contract_ok.mm", source_text);
    let output_base = dir.path().join("callback");
    succeeds(run_build(&input, &output_base, true));

    let artifact = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().and_then(|extension| extension.to_str()) == Some("ll"))
        .expect("callback build must emit LLVM IR");
    let sidecar = artifact.with_extension("semantic-contract.json");
    let sidecar_value: Value = serde_json::from_slice(&fs::read(&sidecar).unwrap()).unwrap();
    assert!(sidecar_value["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|contract| {
            contract["profile"] == PURE_I64_COMPOSITE_PROFILE_V27
                && contract["specification"]["callback_parameters"]
                    .as_array()
                    .is_some_and(|callbacks| !callbacks.is_empty())
        }));
    assert!(sidecar_value["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .all(|contract| {
            let has_callback = contract["specification"]["callback_parameters"]
                .as_array()
                .is_some_and(|callbacks| !callbacks.is_empty());
            if has_callback {
                contract["profile"] == PURE_I64_COMPOSITE_PROFILE_V27
            } else {
                contract["profile"] != PURE_I64_COMPOSITE_PROFILE_V27
            }
        }));
    let apply_contract = sidecar_value["contracts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|contract| contract["atom"] == "apply_nonneg")
        .unwrap();
    let callback = &apply_contract["specification"]["callback_parameters"][0];
    assert!(callback["type"].as_str().unwrap().contains("atom_ref"));
    assert_eq!(callback["requires"], Value::Null);
    assert_eq!(callback["ensures"], "result >= 0");
    assert!(apply_contract["assumptions"]
        .as_array()
        .unwrap()
        .contains(&Value::String(
            "callback_contract_assumed_not_runtime_checked".to_owned()
        )));

    let recheck = succeeds(run_sidecar_recheck(&sidecar, &input, &artifact, true));
    assert_eq!(
        serde_json::from_slice::<Value>(&recheck.stdout).unwrap()["recheck"],
        "consistent"
    );
    let accepted = sidecar_accept_command(&sidecar, &input, &artifact, true)
        .output()
        .unwrap();
    assert!(
        accepted.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&accepted.stdout),
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&accepted.stdout).unwrap()["accepted"],
        true
    );
}

#[test]
fn array_parameter_sidecar_uses_v26_rechecks_accepts_and_binds_signature() {
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "array.mm", ARRAY_PARAM_SOURCE);
    let output_base = dir.path().join("array");
    succeeds(run_build(&input, &output_base, true));

    let artifact = only_llvm_file(dir.path());
    let sidecar = artifact.with_extension("semantic-contract.json");
    let sidecar_bytes = fs::read(&sidecar).unwrap();
    let sidecar_value: Value = serde_json::from_slice(&sidecar_bytes).unwrap();
    assert_eq!(
        sidecar_value["contracts"][0]["profile"],
        PURE_I64_COMPOSITE_PROFILE_V26
    );
    let recheck = succeeds(run_sidecar_recheck(&sidecar, &input, &artifact, true));
    assert_eq!(
        serde_json::from_slice::<Value>(&recheck.stdout).unwrap()["recheck"],
        "consistent"
    );
    let accepted = sidecar_accept_command(&sidecar, &input, &artifact, true)
        .output()
        .unwrap();
    assert!(
        accepted.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&accepted.stdout),
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&accepted.stdout).unwrap()["accepted"],
        true
    );

    let (secret, public) = generate_producer_key(dir.path());
    let signature = dir.path().join("array.signature.json");
    write_signature(&signature, &sidecar, &secret, None);
    let mut command = sidecar_accept_command(&sidecar, &input, &artifact, true);
    command
        .args(["--signature"])
        .arg(&signature)
        .args(["--producer-key", &public])
        .args(["--expected-producer-key", PRODUCER_KEY_ID]);
    let signed_accept = command.output().unwrap();
    assert!(
        signed_accept.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&signed_accept.stdout),
        String::from_utf8_lossy(&signed_accept.stderr)
    );
    let descriptor: Value = serde_json::from_slice(&signed_accept.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert!(descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .contains(&Value::String("producer_signature_binding".to_owned())));
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
fn build_rechecks_and_accepts_inferred_refined_returns_under_v28() {
    const SOURCE: &str = r#"
type Port = i64 where v >= 1 && v <= 65535;
atom validate_port_range(port: Port)
requires: true;
ensures: result >= 1 && result <= 65535;
body: port;
"#;
    let dir = tempfile::tempdir().unwrap();
    let input = write_input(dir.path(), "verified_config.mm", SOURCE);
    let output_base = dir.path().join("verified_config");
    succeeds(run_build(&input, &output_base, true));

    let llvm_path = only_llvm_file(dir.path());
    let sidecar_path = llvm_path.with_extension("semantic-contract.json");
    let sidecar: Value = serde_json::from_slice(&fs::read(&sidecar_path).unwrap()).unwrap();
    assert_eq!(
        sidecar["contracts"][0]["profile"],
        mumei_core::semantic_contract::PURE_I64_COMPOSITE_PROFILE_V28
    );
    assert!(sidecar["contracts"][0]["assumptions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|assumption| assumption == "inferred_refined_return_erased_to_base"));

    succeeds(run_sidecar_recheck(
        &sidecar_path,
        &input,
        &llvm_path,
        false,
    ));
    succeeds(
        sidecar_accept_command(&sidecar_path, &input, &llvm_path, false)
            .output()
            .unwrap(),
    );
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
fn sidecar_recheck_rejects_an_empty_loop_exit_domain_and_accepts_a_bounded_loop_control() {
    let empty_dir = tempfile::tempdir().unwrap();
    let (source, artifact, sidecar) = write_sidecar_fixture(
        empty_dir.path(),
        "main.mm",
        NON_EXITING_REQUIRES_LOOP_SOURCE,
        "empty.ll",
    );

    let output = run_sidecar_recheck(&sidecar, &source, &artifact, true);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("source domain and loop exits empty on all inputs"));

    let accepted = sidecar_accept_command(&sidecar, &source, &artifact, true)
        .output()
        .unwrap();
    assert_eq!(accepted.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&accepted.stderr)
        .contains("source domain and loop exits empty on all inputs"));

    let bounded_dir = tempfile::tempdir().unwrap();
    let (source, artifact, sidecar) = write_sidecar_fixture(
        bounded_dir.path(),
        "main.mm",
        BOUNDED_LOOP_SOURCE,
        "bounded.ll",
    );
    let output = run_sidecar_recheck(&sidecar, &source, &artifact, true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["recheck"], "consistent");
}

#[test]
fn sidecar_recheck_rejects_requires_empty_of_bounded_loop_exits() {
    let dir = tempfile::tempdir().unwrap();
    let (source, artifact, sidecar) = write_sidecar_fixture(
        dir.path(),
        "count.mm",
        REQUIRES_EMPTY_BOUNDED_LOOP_SOURCE,
        "count.ll",
    );

    let output = run_sidecar_recheck(&sidecar, &source, &artifact, true);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("postcondition hypotheses and loop exits empty on all inputs"));

    let accepted = sidecar_accept_command(&sidecar, &source, &artifact, true)
        .output()
        .unwrap();
    assert_eq!(accepted.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&accepted.stderr)
        .contains("postcondition hypotheses and loop exits empty on all inputs"));
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

#[test]
fn accept_sidecar_emits_exact_unsigned_json_and_text_descriptors() {
    let dir = tempfile::tempdir().unwrap();
    let (source, artifact, sidecar) = build_clamp_sidecar(dir.path());
    let sidecar_bytes = fs::read(&sidecar).unwrap();
    let artifact_bytes = fs::read(&artifact).unwrap();
    let contracts = extract_pure_i64_module_contracts(
        CLAMP_SOURCE,
        "clamp.mm",
        None,
        RefinedAdmission::CompositeV24,
    )
    .unwrap();

    let output = sidecar_accept_command(&sidecar, &source, &artifact, true)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_keys(
        &descriptor,
        &[
            "schema_version",
            "sidecar_schema_version",
            "sidecar_fingerprint",
            "artifact_sha256",
            "atoms",
            "acceptance",
            "accepted",
            "scope",
            "checks_performed",
            "checks_not_performed",
        ],
    );
    assert_eq!(
        descriptor["schema_version"],
        "mumei.semantic-contract-sidecar-acceptance/v1"
    );
    assert_eq!(
        descriptor["sidecar_schema_version"],
        "mumei.semantic-contract-sidecar/v1"
    );
    assert_eq!(
        descriptor["sidecar_fingerprint"],
        format!("{:x}", Sha256::digest(&sidecar_bytes))
    );
    assert_eq!(
        descriptor["artifact_sha256"],
        format!("{:x}", Sha256::digest(&artifact_bytes))
    );
    assert_eq!(
        descriptor["atoms"],
        serde_json::to_value(
            contracts
                .iter()
                .map(|contract| contract.atom.clone())
                .collect::<Vec<_>>()
        )
        .unwrap()
    );
    assert_eq!(descriptor["acceptance"], "evaluated");
    assert_eq!(descriptor["accepted"], true);
    assert_eq!(
        descriptor["scope"],
        "semantic_contract_sidecar_hash_binding_and_fresh_source_obligations"
    );
    assert_eq!(
        descriptor["checks_performed"],
        serde_json::json!([
            "sidecar_shape",
            "artifact_hash_binding",
            "contract_fresh_extraction",
            "module_obligations_unsat"
        ])
    );
    assert_eq!(
        descriptor["checks_not_performed"],
        serde_json::json!([
            "producer_authenticity",
            "record_freshness",
            "source_to_llvm_correspondence",
            "artifact_semantics",
            "parser_encoder_solver_correctness",
            "grade_promotion",
            "lean_kernel_checks"
        ])
    );
    assert!(descriptor.get("producer_signature").is_none());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains(dir.path().to_str().unwrap()));
    let key_positions: Vec<_> = [
        "schema_version",
        "sidecar_schema_version",
        "sidecar_fingerprint",
        "artifact_sha256",
        "atoms",
        "acceptance",
        "accepted",
        "scope",
        "checks_performed",
        "checks_not_performed",
    ]
    .iter()
    .map(|key| stdout.find(&format!("\"{key}\":")).unwrap())
    .collect();
    assert!(key_positions.windows(2).all(|pair| pair[0] < pair[1]));

    let output = sidecar_accept_command(&sidecar, &source, &artifact, false)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"Semantic-contract sidecar accepted: shape valid, artifact hash bound, contracts match fresh extraction, module obligations unsat.\n\
Acceptance scope excludes producer authenticity, record freshness, source-to-LLVM correspondence, artifact semantics, grade and final-executable assurance.\n"
    );
}

#[test]
fn accept_sidecar_binds_the_producer_signature_to_sidecar_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (source, artifact, sidecar) = build_clamp_sidecar(dir.path());
    let (secret, public) = generate_producer_key(dir.path());
    let signature = dir.path().join("signature.json");
    write_signature(&signature, &sidecar, &secret, None);

    let mut command = sidecar_accept_command(&sidecar, &source, &artifact, true);
    command
        .args(["--signature"])
        .arg(&signature)
        .args(["--producer-key", &public])
        .args(["--expected-producer-key", PRODUCER_KEY_ID]);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert!(descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .contains(&Value::String("producer_signature_binding".to_owned())));
    let binding = &descriptor["producer_signature"];
    assert_eq!(binding["signature_valid"], true);
    assert_eq!(binding["subject_bound"], true);
    assert_eq!(binding["key_pinned"], true);
    assert!(binding.get("produced_at_signed").is_none());
}

#[test]
fn accept_sidecar_applies_signature_envelope_freshness_without_record_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    let (source, artifact, sidecar) = build_clamp_sidecar(dir.path());
    let (secret, public) = generate_producer_key(dir.path());
    let signature = dir.path().join("signature.json");
    write_signature(&signature, &sidecar, &secret, Some(SIGNED_AT));

    let mut command = sidecar_accept_command(&sidecar, &source, &artifact, true);
    command
        .args(["--signature"])
        .arg(&signature)
        .args(["--producer-key", &public])
        .args(["--expected-producer-key", PRODUCER_KEY_ID])
        .args(["--reference-time", REFERENCE_TIME])
        .args(["--max-age-seconds", "172800"]);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert!(!descriptor["checks_not_performed"]
        .as_array()
        .unwrap()
        .contains(&Value::String("record_freshness".to_owned())));
    assert_eq!(descriptor["producer_signature"]["envelope_fresh"], true);
    assert!(descriptor["producer_signature"]
        .get("produced_at_signed")
        .is_none());
}

#[test]
fn accept_sidecar_rejects_a_signature_over_different_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (source, artifact, sidecar) = build_clamp_sidecar(dir.path());
    let (secret, public) = generate_producer_key(dir.path());
    let other_subject = write_input(dir.path(), "other.json", "different bytes");
    let signature = dir.path().join("signature.json");
    write_signature(&signature, &other_subject, &secret, None);

    let mut command = sidecar_accept_command(&sidecar, &source, &artifact, true);
    command
        .args(["--signature"])
        .arg(&signature)
        .args(["--producer-key", &public])
        .args(["--expected-producer-key", PRODUCER_KEY_ID]);
    let output = command.output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["accepted"], false);
    assert_eq!(descriptor["producer_signature"]["subject_bound"], false);
}

#[test]
fn accept_sidecar_refutes_a_freshly_extracted_false_postcondition() {
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
            "file_name": "unrelated-artifact.ll",
            "sha256": format!("{:x}", Sha256::digest(&artifact_bytes)),
        },
        "artifact_binding": "sha256_of_emitted_file_not_correspondence",
        "correspondence": "unchecked",
        "contracts": serde_json::to_value(contracts).unwrap(),
    });
    let sidecar = dir.path().join("f.semantic-contract.json");
    fs::write(&sidecar, serde_json::to_vec(&sidecar_value).unwrap()).unwrap();

    let output = sidecar_accept_command(&sidecar, &source, &artifact, true)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
}

#[test]
fn accept_sidecar_rejects_a_tampered_artifact_without_descriptor() {
    let dir = tempfile::tempdir().unwrap();
    let (source, artifact, sidecar) = build_clamp_sidecar(dir.path());
    let mut artifact_bytes = fs::read(&artifact).unwrap();
    artifact_bytes.push(b' ');
    fs::write(&artifact, artifact_bytes).unwrap();

    let output = sidecar_accept_command(&sidecar, &source, &artifact, true)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("artifact differs from sidecar hash binding"));
}

#[test]
fn accept_sidecar_rejects_invalid_signature_options_before_source_work() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, sidecar) = build_clamp_sidecar(dir.path());
    let signature = dir.path().join("signature.json");
    let missing_source = dir.path().join("missing.mm");
    let missing_artifact = dir.path().join("missing.ll");

    let output = sidecar_accept_command(&sidecar, &missing_source, &missing_artifact, true)
        .args(["--signature"])
        .arg(signature)
        .args(["--producer-key", "invalid"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid producer public key"));
}

#[test]
fn accept_sidecar_signs_a_real_sidecar_larger_than_the_old_subject_limit() {
    let dir = tempfile::tempdir().unwrap();
    let binder = "x".repeat(1_400);
    let mut source = String::new();
    for index in 0..9 {
        source.push_str(&format!(
            "atom f{index}({binder}: i64) -> i64 requires: {binder} == {binder}; ensures: result == {binder}; body: {binder};\n"
        ));
    }
    source.push_str("atom main() -> i64 requires: true; ensures: result == 0; body: 0;\n");
    assert!(source.len() <= 65_536);

    let input = write_input(dir.path(), "large-contracts.mm", &source);
    let output_base = dir.path().join("large-contracts");
    succeeds(run_build(&input, &output_base, true));

    let artifact = dir.path().join("large-contracts_main.ll");
    assert!(artifact.is_file());
    let sidecar = artifact.with_extension("semantic-contract.json");
    let sidecar_bytes = fs::read(&sidecar).unwrap();
    assert!(
        sidecar_bytes.len() > 65_536,
        "generated sidecar size: {} bytes",
        sidecar_bytes.len()
    );
    println!(
        "generated semantic-contract sidecar: {} bytes",
        sidecar_bytes.len()
    );

    let (secret, public) = generate_producer_key(dir.path());
    let sign_output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "sign"])
        .arg(&sidecar)
        .args(["--key-file"])
        .arg(&secret)
        .args(["--producer-key-id", PRODUCER_KEY_ID, "--json"])
        .output()
        .unwrap();
    assert!(
        sign_output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&sign_output.stdout),
        String::from_utf8_lossy(&sign_output.stderr)
    );
    assert!(sign_output.stderr.is_empty());
    let secret_text = fs::read_to_string(&secret).unwrap();
    assert!(!String::from_utf8_lossy(&sign_output.stdout).contains(&secret_text));
    assert!(!String::from_utf8_lossy(&sign_output.stderr).contains(&secret_text));
    let signature = dir.path().join("large-sidecar.signature.json");
    fs::write(&signature, &sign_output.stdout).unwrap();

    let mut command = sidecar_accept_command(&sidecar, &input, &artifact, true);
    command
        .args(["--signature"])
        .arg(&signature)
        .args(["--producer-key", &public])
        .args(["--expected-producer-key", PRODUCER_KEY_ID]);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(descriptor["accepted"], true);
    assert!(descriptor["checks_performed"]
        .as_array()
        .unwrap()
        .contains(&Value::String("producer_signature_binding".to_owned())));
}
