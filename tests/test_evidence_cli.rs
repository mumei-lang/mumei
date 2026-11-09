use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
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
