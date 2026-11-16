use serde_json::{json, Value};
use std::{
    fs,
    io::{Seek, Write},
    path::Path,
    process::{Command, Output, Stdio},
};

const FIXTURES: &str = include_str!("fixtures/verification-evidence/synthetic_cases.json");
const SCHEMA: &str = include_str!("../schema/verification-evidence.schema.json");
const MATRIX: &str = include_str!("fixtures/verification-evidence/representation_matrix.json");

fn base() -> Value {
    serde_json::from_str::<Value>(FIXTURES).unwrap()["cases"][0]["evidence"].clone()
}

fn command(path: &Path, json: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.arg("evidence").arg("check").arg(path);
    if json {
        command.arg("--json");
    }
    command
}

fn run(path: &Path, json: bool) -> Output {
    command(path, json).output().unwrap()
}

fn run_stdin(body: &[u8], json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.args(["evidence", "check", "--stdin"]);
    if json {
        command.arg("--json");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Err(error) = child.stdin.take().unwrap().write_all(body) {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
    child.wait_with_output().unwrap()
}

#[test]
fn explicit_stdin_matches_file_diagnostics_without_echoing_or_accepting_claims() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FAKE_PRIVATE_RECORD.json");
    let cases: Value = serde_json::from_str(FIXTURES).unwrap();
    for case in cases["cases"].as_array().unwrap() {
        let mut record = case["evidence"].clone();
        record["artifact_paths"] = json!(["FAKE_PRIVATE_NONEXISTENT_PATH"]);
        record["exclusions"] = json!(["FAKE_PRIVATE_JSON"]);
        let body = serde_json::to_vec(&record).unwrap();
        fs::write(&path, &body).unwrap();
        for json in [false, true] {
            let output = run_stdin(&body, json);
            assert_valid(&output, json);
            assert_eq!(output.stdout, run(&path, json).stdout);
        }
    }
}

#[test]
fn stdin_uses_original_bytes_for_duplicate_encoding_and_closed_schema_checks() {
    let mut unknown = base();
    unknown["FAKE_PRIVATE_UNKNOWN"] = json!(true);
    let mut duplicate = serde_json::to_vec(&base()).unwrap();
    duplicate.pop();
    duplicate.extend_from_slice(br#", "result":"established"}"#);
    for body in [
        b"".to_vec(),
        b"{}".to_vec(),
        b"[]".to_vec(),
        b"null".to_vec(),
        b"{\"success\":true}".to_vec(),
        b"{\"FAKE_PRIVATE_JSON\":NaN}".to_vec(),
        b"{\"FAKE_PRIVATE_JSON\":\"\\ud800\"}".to_vec(),
        b"\xff\xfe{}".to_vec(),
        b"\xef\xbb\xbf{}".to_vec(),
        b"{}{}".to_vec(),
        b"{\"FAKE_PRIVATE_JSON\":".to_vec(),
        serde_json::to_vec(&unknown).unwrap(),
        duplicate,
        serde_json::to_vec(&base()["subject"]).unwrap(),
    ] {
        for json in [false, true] {
            assert_invalid(&run_stdin(&body, json), Path::new("FAKE_PRIVATE_INPUT"));
        }
    }
}

#[test]
fn stdin_enforces_exact_byte_boundary_including_trailing_whitespace() {
    let mut body = serde_json::to_vec(&base()).unwrap();
    body.resize(65_536, b' ');
    for json in [false, true] {
        assert_valid(&run_stdin(&body, json), json);
    }
    for size in [65_537, 65_538, 1_048_576] {
        body.resize(size, b' ');
        for json in [false, true] {
            assert_invalid(&run_stdin(&body, json), Path::new("FAKE_PRIVATE_INPUT"));
        }
    }
}

#[test]
fn stdin_never_prefetches_beyond_the_overflow_byte() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oversized.json");
    fs::write(&path, vec![b' '; 1_048_576]).unwrap();
    let mut input = fs::File::open(&path).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "check", "--stdin", "--json"])
        .stdin(Stdio::from(input.try_clone().unwrap()))
        .output()
        .unwrap();
    assert_invalid(&output, &path);
    assert_eq!(input.stream_position().unwrap(), 65_537);
}

#[test]
fn input_selection_is_explicit_exclusive_and_dash_remains_a_file() {
    for args in [
        vec!["evidence", "check"],
        vec!["evidence", "check", "FAKE_PRIVATE_RECORD", "--stdin"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("-");
    write(&path, &base());
    for json in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
        command
            .current_dir(dir.path())
            .args(["evidence", "check", "-"])
            .stdin(Stdio::null());
        if json {
            command.arg("--json");
        }
        assert_valid(&command.output().unwrap(), json);
    }
}

fn assert_valid(output: &Output, json: bool) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    assert!(!text.contains("FAKE_PRIVATE"));
    if json {
        let diagnostic: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            diagnostic,
            json!({
                "schema_version": "mumei.evidence-representation-check/v1",
                "evidence_schema_version": "mumei.verification-evidence/v1",
                "representation": "valid",
                "acceptance": "not_evaluated",
                "checks_not_performed": ["claim_truth", "file_identity", "freshness", "producer_authenticity", "semantic_correspondence", "artifact_semantics"],
            })
        );
    } else {
        assert!(text.contains("Evidence v1 representation valid."));
        assert!(text.contains("Evidence acceptance: not evaluated."));
        assert!(text.contains("no grade promotion or final-executable assurance"));
    }
}

fn assert_invalid(output: &Output, path: &Path) {
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.starts_with("Evidence representation check failed:"));
    assert!(!error.contains("FAKE_PRIVATE"));
    assert!(!error.contains(path.to_str().unwrap()));
}

fn write(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

#[test]
fn all_synthetic_and_native_records_check_without_consuming_their_envelopes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FAKE_PRIVATE_EVIDENCE.json");
    let cases: Value = serde_json::from_str(FIXTURES).unwrap();
    let native: Value = serde_json::from_str(include_str!(
        "fixtures/verification-evidence/native_consumer_shape.json"
    ))
    .unwrap();
    for record in cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|case| &case["evidence"])
        .chain(native["evidence"].as_array().unwrap())
    {
        write(&path, record);
        let before = fs::read(&path).unwrap();
        for json in [false, true] {
            assert_valid(&run(&path, json), json);
        }
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    for envelope in [
        cases,
        native,
        serde_json::from_str(include_str!(
            "fixtures/proof-cert/verified_sample.proof-cert.json"
        ))
        .unwrap(),
    ] {
        write(&path, &envelope);
        for json in [false, true] {
            assert_invalid(&run(&path, json), &path);
        }
    }
}

fn combinations(schema: &Value, paths: &[&str]) -> Vec<Vec<Value>> {
    let mut rows = vec![Vec::new()];
    for path in paths {
        let mut node = schema;
        for key in path.split('.') {
            node = &node["properties"][key];
        }
        rows = rows
            .into_iter()
            .flat_map(|row| {
                node["enum"].as_array().unwrap().iter().map(move |value| {
                    let mut row = row.clone();
                    row.push(value.clone());
                    row
                })
            })
            .collect();
    }
    rows
}

#[test]
fn all_1152_schema_oracle_combinations_have_representation_only_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("matrix.json");
    let oracle: Value = serde_json::from_str(MATRIX).unwrap();
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    let mut valid_diagnostic = None;
    for (section, paths) in [
        (
            "outcomes",
            vec!["method", "execution", "result", "scope.kind"],
        ),
        (
            "correspondence",
            vec!["correspondence.relation", "correspondence.status", "result"],
        ),
    ] {
        let rows = combinations(&schema, &paths);
        assert_eq!(
            rows.len() as u64,
            oracle["case_counts"][section].as_u64().unwrap()
        );
        for (index, row) in rows.iter().enumerate() {
            let mut record = base();
            for (path, value) in paths.iter().zip(row) {
                *record
                    .pointer_mut(&format!("/{}", path.replace('.', "/")))
                    .unwrap() = value.clone();
            }
            if section == "correspondence" {
                record["method"] = json!(match record["result"].as_str().unwrap() {
                    "observed_no_failure" => "finite_execution",
                    "finding" => "abstract_interpretation",
                    _ => "deductive_proof",
                });
                record["scope"]["kind"] = json!(if record["method"] == "finite_execution" {
                    "finite"
                } else {
                    "unbounded"
                });
            }
            record["scope"]["bounds"] = if matches!(
                record["scope"]["kind"].as_str().unwrap(),
                "bounded" | "finite"
            ) {
                json!(["synthetic_limit"])
            } else {
                json!([])
            };
            write(&path, &record);
            let output = run(&path, true);
            let streamed = run_stdin(&fs::read(&path).unwrap(), true);
            assert_eq!(streamed.status.code(), output.status.code());
            assert_eq!(streamed.stdout, output.stdout);
            assert_eq!(streamed.stderr, output.stderr);
            if oracle["valid_indices"][section]
                .as_array()
                .unwrap()
                .contains(&json!(index))
            {
                assert_valid(&output, true);
                assert_eq!(
                    valid_diagnostic.get_or_insert_with(|| output.stdout.clone()),
                    &output.stdout
                );
            } else {
                assert_invalid(&output, &path);
            }
        }
    }
}

#[test]
fn required_nullable_fields_unknown_nested_fields_and_wrong_types_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FAKE_PRIVATE_RECORD.json");
    let original = base();
    for parent in [
        "",
        "/subject",
        "/specification",
        "/semantics",
        "/scope",
        "/correspondence",
        "/provenance",
        "/provenance/adapter",
        "/provenance/tool",
    ] {
        let mut value = original.clone();
        value
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("FAKE_PRIVATE_UNKNOWN".into(), json!("FAKE_PRIVATE_VALUE"));
        write(&path, &value);
        for json in [false, true] {
            assert_invalid(&run(&path, json), &path);
        }
        for key in original
            .pointer(parent)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
        {
            let mut value = original.clone();
            value
                .pointer_mut(parent)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(key);
            write(&path, &value);
            assert_invalid(&run(&path, true), &path);
            for wrong in [json!(false), json!(42), json!([]), json!(null)] {
                if key == "target_triple" && wrong.is_null()
                    || ["bounds", "notes"].contains(&key.as_str()) && wrong == json!([])
                {
                    continue;
                }
                let mut value = original.clone();
                value.pointer_mut(parent).unwrap()[key] = wrong;
                write(&path, &value);
                if ["assumptions", "exclusions", "artifact_paths"].contains(&key.as_str())
                    && value[key] == json!([])
                {
                    assert_valid(&run(&path, true), true);
                    continue;
                }
                assert_invalid(&run(&path, true), &path);
            }
        }
    }
    for target in [json!(null), json!("synthetic-target")] {
        let mut value = original.clone();
        value["semantics"]["target_triple"] = target;
        write(&path, &value);
        assert_valid(&run(&path, true), true);
    }
}

#[test]
fn duplicate_keys_at_every_object_level_including_escaped_and_nullable_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("duplicates.json");
    let original = base();
    let raw = original.to_string();
    for parent in [
        "",
        "/subject",
        "/specification",
        "/semantics",
        "/scope",
        "/correspondence",
        "/provenance",
        "/provenance/adapter",
        "/provenance/tool",
    ] {
        let object = original.pointer(parent).unwrap();
        for (key, value) in object.as_object().unwrap() {
            let inner = object.to_string();
            let duplicated = format!("{{{}:{value},{}", json!(key), &inner[1..]);
            let changed = raw.replacen(&inner, &duplicated, 1);
            assert_ne!(changed, raw);
            fs::write(&path, changed).unwrap();
            for json in [false, true] {
                assert_invalid(&run(&path, json), &path);
            }
        }
    }
    fs::write(
        &path,
        raw.replacen(
            "\"method\":",
            "\"\\u006dethod\":\"deductive_proof\",\"method\":",
            1,
        ),
    )
    .unwrap();
    assert_invalid(&run(&path, true), &path);
}

#[test]
fn malformed_non_utf8_deep_trailing_and_size_boundaries_do_not_emit_partial_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("FAKE_PRIVATE_MALFORMED.json");
    let original = base().to_string();
    for bytes in [
        Vec::new(),
        b"FAKE_PRIVATE_NOT_JSON".to_vec(),
        vec![0xff, 0xfe],
        b"null".to_vec(),
        b"[]".to_vec(),
        format!("{original}{original}").into_bytes(),
        original.as_bytes()[..original.len() / 2].to_vec(),
        format!("{}0{}", "[".repeat(129), "]".repeat(129)).into_bytes(),
    ] {
        fs::write(&path, bytes).unwrap();
        for json in [false, true] {
            assert_invalid(&run(&path, json), &path);
        }
    }
    let mut padded = original.into_bytes();
    padded.resize(65_536, b' ');
    fs::write(&path, &padded).unwrap();
    for json in [false, true] {
        assert_valid(&run(&path, json), json);
    }
    padded.push(b' ');
    fs::write(&path, &padded).unwrap();
    for json in [false, true] {
        assert_invalid(&run(&path, json), &path);
    }
    fs::File::create(&path).unwrap().set_len(1 << 32).unwrap();
    assert_invalid(&run(&path, true), &path);
}

#[test]
fn optional_metadata_wrong_digests_and_duplicate_or_empty_lists_obey_core_rules() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("record.json");
    for optional in [json!(null), json!("FAKE_PRIVATE_TRANSLATOR")] {
        let mut value = base();
        value["provenance"]["translator_version"] = optional;
        value["provenance"]["bridge_lemma_hash"] = json!("f".repeat(64));
        write(&path, &value);
        assert_valid(&run(&path, true), true);
    }
    for field in [
        "/subject/sha256",
        "/specification/sha256",
        "/provenance/configuration_fingerprint",
    ] {
        for digest in [
            "A".repeat(64),
            "g".repeat(64),
            "a".repeat(63),
            "a".repeat(65),
            "FAKE_PRIVATE_DIGEST".into(),
        ] {
            let mut value = base();
            *value.pointer_mut(field).unwrap() = json!(digest);
            write(&path, &value);
            assert_invalid(&run(&path, true), &path);
        }
    }
    for field in [
        "/assumptions",
        "/exclusions",
        "/artifact_paths",
        "/correspondence/notes",
    ] {
        for list in [
            json!([""]),
            json!(["FAKE_PRIVATE_DUPLICATE", "FAKE_PRIVATE_DUPLICATE"]),
        ] {
            let mut value = base();
            *value.pointer_mut(field).unwrap() = list;
            write(&path, &value);
            assert_invalid(&run(&path, true), &path);
        }
    }
}

#[test]
fn static_diagnostic_is_not_evidence_or_a_reusable_acceptance_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("record.json");
    write(&path, &base());
    let output = run(&path, true);
    assert_valid(&output, true);
    fs::write(&path, output.stdout).unwrap();
    assert_invalid(&run(&path, true), &path);
    for key in [
        "acceptance",
        "proved",
        "grade",
        "artifact_verified",
        "lean_verified",
        "environment",
        "command",
    ] {
        let mut value = base();
        value[key] = json!("FAKE_PRIVATE_PROMOTION");
        write(&path, &value);
        assert_invalid(&run(&path, true), &path);
    }
}

#[test]
fn missing_and_directory_inputs_have_static_private_path_free_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("FAKE_PRIVATE_MISSING");
    for path in [missing.as_path(), dir.path()] {
        for json in [false, true] {
            assert_invalid(&run(path, json), path);
        }
    }
}

#[cfg(unix)]
#[test]
fn nonregular_and_symlink_inputs_cannot_block_or_select_another_file() {
    use std::{
        os::unix::{fs::symlink, net::UnixListener},
        process::Stdio,
        time::{Duration, Instant},
    };
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("record.json");
    write(&good, &base());
    let link = dir.path().join("FAKE_PRIVATE_LINK");
    symlink(&good, &link).unwrap();
    let broken = dir.path().join("broken");
    symlink(dir.path().join("absent"), &broken).unwrap();
    let fifo = dir.path().join("FAKE_PRIVATE_FIFO");
    let cpath = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    let socket = dir.path().join("FAKE_PRIVATE_SOCKET");
    let _listener = UnixListener::bind(&socket).unwrap();
    for path in [
        link.as_path(),
        broken.as_path(),
        fifo.as_path(),
        socket.as_path(),
        Path::new("/dev/null"),
    ] {
        let mut child = command(path, true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("nonregular input did not fail promptly");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_invalid(&child.wait_with_output().unwrap(), path);
    }
}

#[cfg(unix)]
#[test]
fn read_only_input_without_tools_never_follows_paths_runs_verifiers_or_writes_claims() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let tools = dir.path().join("tools");
    fs::create_dir(&tools).unwrap();
    let marker = dir.path().join("FAKE_PRIVATE_EXECUTION_MARKER");
    for name in [
        "z3", "llc", "llc-17", "cc", "clang", "lean", "lake", "artifact",
    ] {
        let tool = tools.join(name);
        fs::write(
            &tool,
            format!("#!/bin/sh\nprintf ran > '{}'\nexit 99\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = dir.path().join("FAKE_PRIVATE_INPUT.json");
    let mut value = base();
    value["subject"]["id"] = json!("FAKE_PRIVATE_SUBJECT");
    value["provenance"]["tool"]["name"] = json!(tools.join("z3").to_str().unwrap());
    value["provenance"]["tool"]["version"] = json!("FAKE_PRIVATE_VERSION");
    value["artifact_paths"] = json!([
        tools.join("artifact").to_str().unwrap(),
        "../FAKE_PRIVATE_MISSING",
        "https://FAKE_PRIVATE_USER:FAKE_PRIVATE_PASSWORD@example.invalid/FAKE_PRIVATE_FILE"
    ]);
    write(&path, &value);
    let before = fs::read(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    fs::write(dir.path().join("mumei.toml"), "FAKE_PRIVATE_INVALID_CONFIG").unwrap();
    let count = fs::read_dir(dir.path()).unwrap().count();
    for json in [false, true] {
        let output = command(&path, json)
            .current_dir(dir.path())
            .env("PATH", &tools)
            .env("OPENAI_API_KEY", "FAKE_PRIVATE_SECRET")
            .env(
                "MUMEI_VERIFICATION_TIMEOUT_MS",
                "FAKE_PRIVATE_INVALID_TIMEOUT",
            )
            .env(
                "MUMEI_SOLVER_CONFIG_FINGERPRINT",
                "FAKE_PRIVATE_CONFIGURATION",
            )
            .output()
            .unwrap();
        assert_valid(&output, json);
        assert!(!marker.exists());
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), count);
    }
}

#[test]
fn help_distinguishes_representation_checking_from_proof_and_native_replay() {
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["evidence", "check", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for text in [
        "representation",
        "not its truth",
        "65,536",
        "not a native report envelope",
        "not an accepted Evidence record",
    ] {
        assert!(help.contains(text), "missing help contract: {text}");
    }
}

fn list_envelope(elements: Vec<Value>) -> Value {
    json!({"schema_version": "mumei.verification-evidence-list/v1", "evidence": elements})
}

fn check_list(path: &Path, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.arg("evidence").arg("check-list").arg(path);
    if json {
        command.arg("--json");
    }
    command.output().unwrap()
}

#[test]
fn check_list_admits_a_bounded_envelope_of_records_with_a_count_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    for (count, json_mode) in [(0, true), (2, true), (2, false)] {
        let path = dir.path().join("evidence-list.json");
        write(&path, &list_envelope(vec![base(); count]));
        let output = check_list(&path, json_mode);
        assert!(output.status.success(), "stderr: {:?}", output.stderr);
        if json_mode {
            let diagnostic: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                diagnostic["schema_version"],
                "mumei.evidence-representation-check/v1"
            );
            assert_eq!(
                diagnostic["evidence_schema_version"],
                "mumei.verification-evidence-list/v1"
            );
            assert_eq!(diagnostic["representation"], "valid");
            assert_eq!(diagnostic["record_count"], count);
            assert_eq!(diagnostic["acceptance"], "not_evaluated");
            assert!(diagnostic["checks_not_performed"]
                .as_array()
                .unwrap()
                .contains(&json!("cross_record_consistency")));
        } else {
            let text = String::from_utf8(output.stdout).unwrap();
            assert!(text.contains("2 record(s)"));
            assert!(text.contains("not evaluated"));
        }
    }
}

#[test]
fn check_list_rejects_single_records_bad_elements_and_envelope_abuse() {
    let dir = tempfile::tempdir().unwrap();
    let cases: Vec<Value> = vec![
        // A standalone record is not a list envelope.
        base(),
        // Wrong envelope schema, missing keys, unknown keys, oversized fan-out.
        json!({"schema_version": "mumei.verification-evidence/v1", "evidence": [base()]}),
        json!({"schema_version": "mumei.verification-evidence-list/v1"}),
        json!({"schema_version": "mumei.verification-evidence-list/v1", "evidence": [], "extra": null}),
        list_envelope(vec![base(); 65]),
        // One malformed element poisons the whole envelope.
        list_envelope(vec![base(), json!({"shape": "not-a-record"})]),
        list_envelope(vec![{
            let mut bad = base();
            bad["method"] = json!("FAKE_PRIVATE_METHOD");
            bad
        }]),
    ];
    for (index, value) in cases.iter().enumerate() {
        let path = dir.path().join(format!("list-{index}.json"));
        write(&path, value);
        let output = check_list(&path, true);
        assert!(!output.status.success(), "case {index} unexpectedly passed");
        assert!(output.stdout.is_empty(), "case {index} emitted stdout");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.starts_with("Evidence list representation check failed:"));
        assert!(!error.contains("FAKE_PRIVATE"), "case {index}: {error}");
    }
    // The single-record gate does not recognize the list envelope either.
    let path = dir.path().join("as-single.json");
    write(&path, &list_envelope(vec![base()]));
    let output = run(&path, true);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn check_list_supports_stdin_and_still_emits_no_acceptance_receipt() {
    let body = serde_json::to_vec(&list_envelope(vec![base(), base()])).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.args(["evidence", "check-list", "--stdin", "--json"]);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&body).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let diagnostic: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(diagnostic["record_count"], 2);
    assert!(diagnostic.get("accepted").is_none());
    assert!(diagnostic.get("evidence").is_none());
}

fn check_consistency(path: &Path, json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command.arg("evidence").arg("check-consistency").arg(path);
    if json {
        command.arg("--json");
    }
    command.output().unwrap()
}

#[test]
fn check_consistency_reports_identical_and_independent_records_as_consistent() {
    let dir = tempfile::tempdir().unwrap();
    // Identical duplicates and records sharing no pin are both consistent:
    // the second record names a different subject and obligation.
    let mut independent = base();
    independent["subject"]["id"] = json!("other.mm");
    independent["subject"]["sha256"] = json!("b".repeat(64));
    independent["obligation_id"] = json!("other-obligation");
    for elements in [
        vec![base()],
        vec![base(), base()],
        vec![base(), independent],
    ] {
        let path = dir.path().join("list.json");
        write(&path, &list_envelope(elements));
        for json_mode in [true, false] {
            let output = check_consistency(&path, json_mode);
            assert!(output.status.success(), "stderr: {:?}", output.stderr);
            if json_mode {
                let diagnostic: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(
                    diagnostic["schema_version"],
                    "mumei.evidence-consistency-check/v1"
                );
                assert_eq!(diagnostic["consistent"], true);
                assert_eq!(diagnostic["conflicts"], json!([]));
                assert_eq!(diagnostic["acceptance"], "not_evaluated");
            }
        }
    }
}

#[test]
fn check_consistency_names_conflicting_identity_pins() {
    let dir = tempfile::tempdir().unwrap();
    // Two records claiming the same obligation but different subject content
    // are a named conflict. The shared subject.id is only a producer label
    // (standalone source records all use "source.mm"), so the obligation
    // rule — not the subject-id rule — reports the divergence.
    let mut drifted = base();
    drifted["subject"]["sha256"] = json!("a".repeat(64));
    let path = dir.path().join("list.json");
    write(&path, &list_envelope(vec![base(), drifted]));
    let output = check_consistency(&path, true);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let diagnostic: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(diagnostic["consistent"], false);
    assert_eq!(diagnostic["conflict_count"], 1);
    let conflicts = diagnostic["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["field"], "subject.sha256");
    assert_eq!(conflicts[0]["index_a"], 0);
    assert_eq!(conflicts[0]["index_b"], 1);

    // Same obligation_id with a different specification pin is a conflict,
    // even with no shared subject.
    let mut rebound = base();
    rebound["subject"]["id"] = json!("other.mm");
    rebound["subject"]["sha256"] = json!("b".repeat(64));
    rebound["specification"]["sha256"] = json!("c".repeat(64));
    write(&path, &list_envelope(vec![base(), rebound]));
    let diagnostic: Value = serde_json::from_slice(&check_consistency(&path, true).stdout).unwrap();
    assert_eq!(diagnostic["consistent"], false);
    assert_eq!(diagnostic["conflict_count"], 3);
    let fields: Vec<&str> = diagnostic["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|conflict| conflict["field"].as_str().unwrap())
        .collect();
    assert!(fields.contains(&"subject.id"));
    assert!(fields.contains(&"specification.sha256"));
}

#[test]
fn check_consistency_rejects_malformed_envelopes_like_check_list() {
    let dir = tempfile::tempdir().unwrap();
    for (index, value) in [
        base(),
        list_envelope(vec![base(), json!({"shape": "not-a-record"})]),
        list_envelope(vec![base(); 65]),
    ]
    .iter()
    .enumerate()
    {
        let path = dir.path().join(format!("list-{index}.json"));
        write(&path, value);
        let output = check_consistency(&path, true);
        assert!(!output.status.success(), "case {index} unexpectedly passed");
        assert!(output.stdout.is_empty(), "case {index} emitted stdout");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.starts_with("Evidence consistency check failed:"));
        assert!(!error.contains("FAKE_PRIVATE"), "case {index}: {error}");
    }
}

#[test]
fn expected_adapter_gates_every_check_command_on_the_declared_producer_only() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(base()["provenance"]["adapter"]["id"], "synthetic-adapter");
    let single = dir.path().join("record.json");
    write(&single, &base());
    let list = dir.path().join("list.json");
    write(&list, &list_envelope(vec![base(), base()]));

    let commands: Vec<(&str, &Path, &str)> = vec![
        ("check", &single, "Evidence representation check failed:"),
        (
            "check-list",
            &list,
            "Evidence list representation check failed:",
        ),
        (
            "check-consistency",
            &list,
            "Evidence consistency check failed:",
        ),
    ];
    for (name, path, prefix) in &commands {
        for json in [false, true] {
            let mut matched = Command::new(env!("CARGO_BIN_EXE_mumei"));
            matched
                .args(["evidence", name])
                .arg(path)
                .args(["--expected-adapter", "synthetic-adapter"]);
            if json {
                matched.arg("--json");
            }
            let output = matched.output().unwrap();
            assert!(
                output.status.success(),
                "{name} json={json} rejected a matching adapter: {:?}",
                output.stderr
            );
            if json {
                let diagnostic: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(
                    diagnostic["checks_performed"]
                        .as_array()
                        .unwrap()
                        .last()
                        .unwrap(),
                    "declared_producer_identity"
                );
                assert!(diagnostic["checks_not_performed"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("producer_authenticity")));
            } else {
                let text = String::from_utf8(output.stdout).unwrap();
                assert!(text.contains("Declared producer identity"), "{name}");
            }

            let mut mismatched = Command::new(env!("CARGO_BIN_EXE_mumei"));
            mismatched
                .args(["evidence", name])
                .arg(path)
                .args(["--expected-adapter", "other-adapter"]);
            if json {
                mismatched.arg("--json");
            }
            let output = mismatched.output().unwrap();
            assert!(!output.status.success(), "{name} json={json}");
            assert!(output.stdout.is_empty(), "{name} emitted stdout");
            let error = String::from_utf8(output.stderr).unwrap();
            assert!(error.starts_with(prefix), "{name}: {error}");
            assert!(error.contains("expected adapter"), "{name}: {error}");
            assert!(!error.contains("synthetic-adapter"), "{name}: {error}");

            let mut empty = Command::new(env!("CARGO_BIN_EXE_mumei"));
            empty
                .args(["evidence", name])
                .arg(path)
                .args(["--expected-adapter", ""]);
            let output = empty.output().unwrap();
            assert!(!output.status.success(), "{name} accepted empty adapter");
            let error = String::from_utf8(output.stderr).unwrap();
            assert!(error.starts_with(prefix), "{name}: {error}");
        }
    }

    // One record declaring another producer poisons the whole list.
    let mut foreign = base();
    foreign["provenance"]["adapter"]["id"] = json!("other-adapter");
    let mixed = dir.path().join("mixed.json");
    write(&mixed, &list_envelope(vec![base(), foreign]));
    for name in ["check-list", "check-consistency"] {
        let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args(["evidence", name])
            .arg(&mixed)
            .args(["--expected-adapter", "synthetic-adapter"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{name} passed a poisoned list");
        assert!(output.stdout.is_empty(), "{name} emitted stdout");
    }
}
