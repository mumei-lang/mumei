use mumei_core::adapter_manifest;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Output, Stdio};

const MINIMAL: &str = include_str!("fixtures/adapter_manifest_minimal.json");
const USER_FIXTURE: &str = include_str!("fixtures/adapter_manifest_user_fixture.json");
const SENTINEL: &str = "MUMEI_MANIFEST_FIELD_SENTINEL_81b5a9";

fn run(args: &[&str], stdin_bytes: Option<&[u8]>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mumei"));
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin_bytes.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    let mut child = command.spawn().expect("spawn mumei");
    if let Some(bytes) = stdin_bytes {
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(bytes)
            .expect("write stdin");
    }
    child.wait_with_output().expect("wait for mumei")
}

fn stdin_check(bytes: &[u8]) -> Output {
    run(
        &["evidence", "check-adapter", "--stdin", "--json"],
        Some(bytes),
    )
}

fn json_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("encode test manifest")
}

fn base_manifest() -> Value {
    serde_json::from_str(MINIMAL).expect("minimal agent fixture")
}

fn with(mut value: Value, key: &str, replacement: Value) -> Value {
    let _ = value
        .as_object_mut()
        .expect("manifest object")
        .insert(key.to_owned(), replacement);
    value
}

fn remove(mut value: Value, key: &str) -> Value {
    let _ = value.as_object_mut().expect("manifest object").remove(key);
    value
}

fn contract() -> Value {
    json!({
        "profile": "mumei.pure-i64-comparison/v1",
        "method": "deductive_proof",
        "scope": "unbounded",
        "correspondence_relation": "exact_model",
        "correspondence_status": "assumed",
    })
}

fn integer_spec() -> Value {
    json!({
        "name": "depth",
        "kind": "integer",
        "default": 4,
        "minimum": 1,
        "maximum": 16,
    })
}

fn marker() -> Value {
    json!({
        "stream": "stdout",
        "contains": "declared-marker",
        "outcome": "valid",
    })
}

fn invalid_manifests() -> Vec<(&'static str, Value)> {
    let mut cases = vec![
        (
            "wrong schema",
            with(base_manifest(), "schema_version", json!("other/v1")),
        ),
        ("missing schema", remove(base_manifest(), "schema_version")),
        (
            "unknown root field",
            with(base_manifest(), "command", json!("python")),
        ),
        ("missing required field", remove(base_manifest(), "methods")),
        (
            "invalid adapter id",
            with(base_manifest(), "adapter_id", json!("not an id")),
        ),
        (
            "invalid adapter version",
            with(base_manifest(), "adapter_version", json!("v1")),
        ),
        (
            "empty profiles",
            with(base_manifest(), "profiles", json!([])),
        ),
        (
            "malformed profile list",
            with(base_manifest(), "profiles", json!(["good", 1])),
        ),
        (
            "invalid profile label",
            with(base_manifest(), "profiles", json!(["bad profile"])),
        ),
        (
            "unsupported method",
            with(base_manifest(), "methods", json!(["magic"])),
        ),
        (
            "null target list",
            with(base_manifest(), "targets", Value::Null),
        ),
        (
            "invalid logic fragment",
            with(base_manifest(), "logic_fragments", json!(["bad fragment"])),
        ),
        (
            "invalid input format",
            with(base_manifest(), "input_formats", json!(["bad format"])),
        ),
        (
            "invalid protocol version",
            with(base_manifest(), "protocol_version", json!("bad protocol")),
        ),
        (
            "boolean priority",
            with(base_manifest(), "priority", json!(true)),
        ),
        (
            "floating-point priority",
            with(base_manifest(), "priority", json!(1.0)),
        ),
        (
            "non-boolean enabled",
            with(base_manifest(), "enabled", json!(1)),
        ),
        (
            "unsupported location",
            with(base_manifest(), "location", json!("sandbox")),
        ),
        (
            "zero timeout",
            with(base_manifest(), "max_timeout_ms", json!(0)),
        ),
        (
            "boolean timeout",
            with(base_manifest(), "max_timeout_ms", json!(true)),
        ),
        (
            "floating-point timeout",
            with(base_manifest(), "max_timeout_ms", json!(1.0)),
        ),
        (
            "zero memory",
            with(base_manifest(), "max_memory_mb", json!(0)),
        ),
        (
            "negative memory",
            with(base_manifest(), "max_memory_mb", json!(-1)),
        ),
        (
            "boolean memory",
            with(base_manifest(), "max_memory_mb", json!(true)),
        ),
        (
            "floating-point memory",
            with(base_manifest(), "max_memory_mb", json!(1.0)),
        ),
        (
            "non-list contracts",
            with(base_manifest(), "result_contracts", json!("contract")),
        ),
        (
            "malformed contract",
            with(base_manifest(), "result_contracts", json!(["contract"])),
        ),
        (
            "unknown contract field",
            with(
                base_manifest(),
                "result_contracts",
                json!([{ "profile":"mumei.pure-i64-comparison/v1",
                    "method":"deductive_proof","scope":"unbounded",
                    "correspondence_relation":"exact_model",
                    "correspondence_status":"assumed","unknown":1 }]),
            ),
        ),
        (
            "missing contract field",
            with(
                base_manifest(),
                "result_contracts",
                json!([{ "profile":"mumei.pure-i64-comparison/v1",
                    "method":"deductive_proof","scope":"unbounded",
                    "correspondence_relation":"exact_model" }]),
            ),
        ),
        (
            "contract outside capabilities",
            with(
                base_manifest(),
                "result_contracts",
                json!([{ "profile":"mumei.other/v1","method":"deductive_proof",
                    "scope":"unbounded","correspondence_relation":"exact_model",
                    "correspondence_status":"assumed" }]),
            ),
        ),
        (
            "contract scope and bounds disagree",
            with(
                base_manifest(),
                "result_contracts",
                json!([{ "profile":"mumei.pure-i64-comparison/v1",
                    "method":"deductive_proof","scope":"bounded",
                    "correspondence_relation":"exact_model",
                    "correspondence_status":"assumed" }]),
            ),
        ),
        (
            "unsupported correspondence status",
            with(
                base_manifest(),
                "result_contracts",
                json!([{ "profile":"mumei.pure-i64-comparison/v1",
                    "method":"deductive_proof","scope":"unbounded",
                    "correspondence_relation":"exact_model",
                    "correspondence_status":"unknown" }]),
            ),
        ),
        (
            "duplicate result contract",
            with(
                base_manifest(),
                "result_contracts",
                json!([contract(), contract()]),
            ),
        ),
        (
            "non-list setting specs",
            with(base_manifest(), "setting_specs", json!("spec")),
        ),
        (
            "unknown setting-spec field",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"depth","kind":"integer","default":4,
                    "minimum":1,"maximum":16,"unknown":true }]),
            ),
        ),
        (
            "missing setting-spec field",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"depth","kind":"integer","minimum":1,"maximum":16 }]),
            ),
        ),
        (
            "invalid setting name",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"Depth","kind":"integer","default":4,
                    "minimum":1,"maximum":16 }]),
            ),
        ),
        (
            "reserved setting name",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"api_key","kind":"boolean","default":true }]),
            ),
        ),
        (
            "unsupported setting kind",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"depth","kind":"text","default":"4" }]),
            ),
        ),
        (
            "integer setting missing bounds",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"depth","kind":"integer","default":4 }]),
            ),
        ),
        (
            "integer setting reversed bounds",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"depth","kind":"integer","default":4,
                    "minimum":10,"maximum":1 }]),
            ),
        ),
        (
            "integer setting boolean default",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"depth","kind":"integer","default":true,
                    "minimum":0,"maximum":10 }]),
            ),
        ),
        (
            "integer setting default outside bounds",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"depth","kind":"integer","default":11,
                    "minimum":0,"maximum":10 }]),
            ),
        ),
        (
            "integer setting float bound",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"depth","kind":"integer","default":4,
                    "minimum":1.0,"maximum":16 }]),
            ),
        ),
        (
            "integer setting bound outside signed 64-bit range",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"depth","kind":"integer","default":4,
                    "minimum":0,"maximum":9223372036854775808u64 }]),
            ),
        ),
        (
            "boolean setting with bound",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"strict","kind":"boolean","default":true,
                    "minimum":0 }]),
            ),
        ),
        (
            "boolean setting non-boolean default",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"strict","kind":"boolean","default":1 }]),
            ),
        ),
        (
            "choice setting without choices",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"mode","kind":"choice","default":"fast" }]),
            ),
        ),
        (
            "choice setting default outside choices",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"mode","kind":"choice","default":"slow",
                    "choices":["fast"] }]),
            ),
        ),
        (
            "choice setting invalid label",
            with(
                base_manifest(),
                "setting_specs",
                json!([{ "name":"mode","kind":"choice","default":"bad value",
                    "choices":["bad value"] }]),
            ),
        ),
        (
            "duplicate setting names",
            with(
                base_manifest(),
                "setting_specs",
                json!([integer_spec(), integer_spec()]),
            ),
        ),
        (
            "unsupported permission",
            with(
                base_manifest(),
                "required_permissions",
                json!(["filesystem.execute"]),
            ),
        ),
        (
            "malformed permission list",
            with(
                base_manifest(),
                "required_permissions",
                json!(["process.spawn", 1]),
            ),
        ),
        (
            "empty probe argv",
            with(base_manifest(), "probe_argv", json!([])),
        ),
        (
            "non-list probe argv",
            with(base_manifest(), "probe_argv", json!("python")),
        ),
        (
            "invalid probe argv item",
            with(base_manifest(), "probe_argv", json!(["python", ""])),
        ),
        (
            "non-string probe argv item",
            with(base_manifest(), "probe_argv", json!(["python", 1])),
        ),
        (
            "empty attempt argv",
            with(base_manifest(), "attempt_argv", json!([])),
        ),
        (
            "invalid attempt argv item",
            with(base_manifest(), "attempt_argv", json!(["python", ""])),
        ),
        (
            "non-string attempt argv item",
            with(base_manifest(), "attempt_argv", json!(["python", 1])),
        ),
        (
            "non-string attempt stdin",
            with(base_manifest(), "attempt_stdin", json!(1)),
        ),
        (
            "non-list verdict markers",
            with(base_manifest(), "verdict_markers", json!("marker")),
        ),
        (
            "too many verdict markers",
            with(
                base_manifest(),
                "verdict_markers",
                json!(vec![marker(); 33]),
            ),
        ),
        (
            "invalid verdict-marker stream",
            with(
                base_manifest(),
                "verdict_markers",
                json!([{ "stream":"stdin","contains":"match","outcome":"valid" }]),
            ),
        ),
        (
            "invalid verdict-marker outcome",
            with(
                base_manifest(),
                "verdict_markers",
                json!([{ "stream":"stdout","contains":"match","outcome":"unknown" }]),
            ),
        ),
        (
            "empty verdict-marker content",
            with(
                base_manifest(),
                "verdict_markers",
                json!([{ "stream":"stdout","contains":"","outcome":"valid" }]),
            ),
        ),
        (
            "oversized verdict-marker content",
            with(
                base_manifest(),
                "verdict_markers",
                json!([{ "stream":"stdout","contains": "x".repeat(257),
                    "outcome":"valid" }]),
            ),
        ),
        (
            "extra verdict-marker field",
            with(
                base_manifest(),
                "verdict_markers",
                json!([{ "stream":"stdout","contains":"match","outcome":"valid",
                    "extra":true }]),
            ),
        ),
    ];
    cases.push((
        "oversized attempt stdin",
        with(base_manifest(), "attempt_stdin", json!("x".repeat(65_537))),
    ));
    cases.push((
        "setting schema exceeds limit",
        with(
            base_manifest(),
            "setting_specs",
            json!((0..33)
                .map(|index| json!({
                    "name": format!("setting{index}"),
                    "kind": "boolean",
                    "default": true,
                }))
                .collect::<Vec<_>>()),
        ),
    ));
    cases.push((
        "choice list exceeds limit",
        with(
            base_manifest(),
            "setting_specs",
            json!([{
                "name":"mode","kind":"choice","default":"choice0",
                "choices":(0..33).map(|index| format!("choice{index}"))
                    .collect::<Vec<_>>(),
            }]),
        ),
    ));
    cases
}

#[test]
fn accepts_agent_fixtures_with_identical_file_and_stdin_diagnostics() {
    let directory = tempfile::tempdir().expect("temporary fixture directory");
    for (name, fixture, argv, stdin, marker_count) in [
        ("minimal", MINIMAL, "absent", "absent", 0),
        ("user fixture", USER_FIXTURE, "declared", "declared", 2),
    ] {
        let path = directory.path().join(format!("{name}.json"));
        std::fs::write(&path, fixture).expect("write fixture");
        let path = path.to_str().expect("UTF-8 temporary path");
        let file_output = run(&["evidence", "check-adapter", path, "--json"], None);
        let stdin_output = stdin_check(fixture.as_bytes());
        assert!(file_output.status.success(), "{name} file fixture rejected");
        assert!(
            stdin_output.status.success(),
            "{name} stdin fixture rejected"
        );

        let diagnostic: Value =
            serde_json::from_slice(&file_output.stdout).expect("JSON diagnostic");
        let output = String::from_utf8_lossy(&file_output.stdout);
        assert!(!output.contains("user.fixture"));
        assert!(!output.contains(SENTINEL));
        for bytes in [
            &file_output.stderr,
            &stdin_output.stdout,
            &stdin_output.stderr,
        ] {
            let output = String::from_utf8_lossy(bytes);
            assert!(!output.contains("user.fixture"));
            assert!(!output.contains(SENTINEL));
        }
        assert_eq!(file_output.stdout, stdin_output.stdout, "{name}");
        assert_eq!(
            diagnostic,
            json!({
                "schema_version": "mumei.adapter-manifest-check/v1",
                "manifest_schema_version": "mumei.verifier-adapter/v1",
                "representation": "valid",
                "acceptance": "not_evaluated",
                "attempt_argv": argv,
                "attempt_stdin": stdin,
                "verdict_marker_count": marker_count,
                "declared_field_meaning": {
                    "attempt_argv": "inert_without_host_opt_in",
                    "attempt_stdin": "inert_without_host_opt_in",
                    "verdict_markers": "adapter_self_report_vocabulary",
                    "max_memory_mb": "declared_ceiling_not_enforced_by_core",
                },
                "checks_performed": ["representation_gate", "manifest_consistency"],
                "checks_not_performed": [
                    "adapter_execution",
                    "command_safety",
                    "stdin_semantics",
                    "reported_outcome_truth",
                    "resource_enforcement",
                    "producer_authenticity",
                    "host_policy",
                    "grade_promotion",
                ],
            })
        );
    }
}

#[test]
fn text_diagnostic_is_one_static_line() {
    let output = run(
        &["evidence", "check-adapter", "--stdin"],
        Some(MINIMAL.as_bytes()),
    );
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 text"),
        "Adapter manifest v1 representation valid; acceptance not evaluated.\n"
    );
}

#[test]
fn rejects_every_manifest_rule_group() {
    for (label, value) in invalid_manifests() {
        let bytes = json_bytes(&value);
        assert!(
            adapter_manifest::decode(&bytes).is_err(),
            "{label} was accepted"
        );
    }
}

#[test]
fn manifest_lists_collapse_duplicates_and_python_big_integers_are_preserved() {
    let mut duplicated = base_manifest();
    duplicated["profiles"] = json!([
        "mumei.pure-i64-comparison/v1",
        "mumei.pure-i64-comparison/v1"
    ]);
    duplicated["methods"] = json!(["deductive_proof", "deductive_proof"]);
    duplicated["logic_fragments"] = json!(["lia", "lia"]);
    duplicated["targets"] = json!(["x86_64-linux", "x86_64-linux"]);
    duplicated["input_formats"] = json!(["mumei.source-text/v1", "mumei.source-text/v1"]);
    duplicated["required_permissions"] = json!(["process.spawn", "process.spawn"]);
    duplicated["result_contracts"] = json!([{
        "profile": "mumei.pure-i64-comparison/v1",
        "method": "deductive_proof",
        "scope": "bounded",
        "bounds": ["user.depth/v1", "user.depth/v1"],
        "correspondence_relation": "exact_model",
        "correspondence_status": "assumed",
    }]);
    duplicated["setting_specs"] = json!([{
        "name": "mode",
        "kind": "choice",
        "default": "fast",
        "choices": ["fast", "fast"],
    }]);
    assert!(adapter_manifest::decode(&json_bytes(&duplicated)).is_ok());

    let huge = "9".repeat(100);
    let raw = format!(
        r#"{{"schema_version":"mumei.verifier-adapter/v1","adapter_id":"fixture","adapter_version":"1.0.0","profiles":["mumei.pure-i64-comparison/v1"],"methods":["deductive_proof"],"priority":{huge},"max_memory_mb":{huge},"max_timeout_ms":{huge}}}"#
    );
    assert!(adapter_manifest::decode(raw.as_bytes()).is_ok());
}

#[test]
fn attempt_stdin_utf8_and_argv_have_no_uninvented_length_limit() {
    let mut manifest = base_manifest();
    manifest["attempt_stdin"] = json!("é".repeat(1_000));
    assert!(adapter_manifest::decode(&json_bytes(&manifest)).is_ok());

    manifest = base_manifest();
    manifest["attempt_argv"] = json!((0..4096).map(|_| "arg").collect::<Vec<_>>());
    assert!(adapter_manifest::decode(&json_bytes(&manifest)).is_ok());
    manifest["attempt_argv"] = json!(["x".repeat(10_000)]);
    assert!(adapter_manifest::decode(&json_bytes(&manifest)).is_ok());
}

#[test]
fn rejects_escaped_lone_surrogate_argv_items() {
    let probe = r#"{"schema_version":"mumei.verifier-adapter/v1","adapter_id":"fixture","adapter_version":"1.0.0","profiles":["mumei.pure-i64-comparison/v1"],"methods":["deductive_proof"],"probe_argv":["\ud800"]}"#;
    let attempt = r#"{"schema_version":"mumei.verifier-adapter/v1","adapter_id":"fixture","adapter_version":"1.0.0","profiles":["mumei.pure-i64-comparison/v1"],"methods":["deductive_proof"],"attempt_argv":["\ud800"]}"#;
    assert!(adapter_manifest::decode(probe.as_bytes()).is_err());
    assert!(adapter_manifest::decode(attempt.as_bytes()).is_err());
}

#[test]
fn rejects_duplicate_keys_invalid_utf8_non_objects_and_oversized_cli_input() {
    let duplicate = br#"{"schema_version":"mumei.verifier-adapter/v1","schema_version":"mumei.verifier-adapter/v1","adapter_id":"fixture","adapter_version":"1.0.0","profiles":["mumei.pure-i64-comparison/v1"],"methods":["deductive_proof"]}"#;
    let duplicate_nested = br#"{"schema_version":"mumei.verifier-adapter/v1","adapter_id":"fixture","adapter_version":"1.0.0","profiles":["mumei.pure-i64-comparison/v1"],"methods":["deductive_proof"],"verdict_markers":[{"stream":"stdout","stream":"stderr","contains":"match","outcome":"valid"}]}"#;
    let invalid_utf8 = [0xff];
    let invalid = [
        ("duplicate key", duplicate.as_slice()),
        ("duplicate nested key", duplicate_nested.as_slice()),
        ("non-object", br#"[]"#.as_slice()),
        ("invalid UTF-8", &invalid_utf8[..]),
    ];
    for (label, bytes) in invalid {
        let output = stdin_check(bytes);
        assert!(!output.status.success(), "{label} was accepted");
        assert!(output.stdout.is_empty(), "{label} wrote success output");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("invalid adapter-manifest representation"),
            "{label} did not use a static error: {:?}",
            output.stderr
        );
    }

    let oversized = vec![b' '; adapter_manifest::MAX_INPUT_BYTES + 1];
    let output = run(&["evidence", "check-adapter", "--stdin"], Some(&oversized));
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr)
        .starts_with("Adapter-manifest representation check failed:"));
}

#[test]
fn invalid_manifest_errors_do_not_echo_input_content() {
    let input = format!(
        r#"{{"schema_version":"mumei.verifier-adapter/v1","adapter_id":"{SENTINEL}","adapter_version":"1.0.0","profiles":["mumei.pure-i64-comparison/v1"],"methods":["deductive_proof"],"unknown":"{SENTINEL}"}}"#
    );
    let output = stdin_check(input.as_bytes());
    assert!(!output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains(SENTINEL));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(SENTINEL));
}
