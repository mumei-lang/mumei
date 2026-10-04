use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

fn verify_source(name: &str, source: &str, extra_args: &[&str]) -> (PathBuf, Output, Value) {
    let dir = std::env::temp_dir().join(format!(
        "mumei_counterexample_raising_{}_{}_{}",
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
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .args(extra_args)
        .arg("--report-dir")
        .arg(&dir)
        .arg(&fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run mumei verify");
    let report = std::fs::read(dir.join("report.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    (dir, output, report)
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn cleanup(dir: PathBuf) {
    std::fs::remove_dir_all(dir).expect("remove fixture directory");
}

fn lsp_diagnostic_for_source(name: &str, source: &str) -> Value {
    let dir = std::env::temp_dir().join(format!(
        "mumei_counterexample_raising_lsp_{}_{}_{}",
        std::process::id(),
        name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create LSP fixture directory");
    let source_path = dir.join(format!("{name}.mm"));
    std::fs::write(&source_path, source).expect("write LSP fixture");
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source_path.display()),
                "languageId": "mumei",
                "version": 1,
                "text": source
            }
        }
    });
    let body = serde_json::to_string(&body).expect("serialize didOpen");
    let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
    let mut child = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("lsp")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn LSP");
    child
        .stdin
        .as_mut()
        .expect("open LSP stdin")
        .write_all(frame.as_bytes())
        .expect("write didOpen");
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait for LSP");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let diagnostic = stdout
        .split("Content-Length: ")
        .skip(1)
        .filter_map(|chunk| chunk.split_once("\r\n\r\n").map(|(_, body)| body))
        .filter_map(|body| {
            serde_json::Deserializer::from_str(body)
                .into_iter::<Value>()
                .next()
        })
        .filter_map(Result::ok)
        .find_map(|message| {
            (message.get("method").and_then(Value::as_str)
                == Some("textDocument/publishDiagnostics"))
            .then_some(message)
        })
        .and_then(|message| {
            message
                .pointer("/params/diagnostics")
                .and_then(Value::as_array)
                .and_then(|diagnostics| {
                    diagnostics
                        .iter()
                        .find(|diagnostic| diagnostic.pointer("/data/counterexample").is_some())
                })
                .cloned()
        })
        .expect("call-site diagnostic includes counterexample data");
    std::fs::remove_dir_all(&dir).expect("remove LSP fixture directory");
    diagnostic
}

const CHECK_CALLEE: &str = r#"
struct Pt { a: i64, b: i64 }

atom check(p: Pt) -> i64
requires: p.a > 0;
ensures: true;
body: p.a;
"#;

const CHECK_INT_CALLEE: &str = r#"
atom check_int(n: i64) -> i64
requires: n > 0;
ensures: true;
body: n;
"#;

const MK_I_CALLEE: &str = r#"
atom mk_i(x: i64) -> i64
requires: true;
ensures: true;
body: x;
"#;

const ABSTRACT_CALL_RESULT_REASON: &str =
    "argument comes from an abstract callee result; its value is not tied to source values";

fn assert_callsite_argument_unraisable(diagnostic: &Value, source_name: &str) {
    let status_path = format!("/data/counterexample_provenance/values/{source_name}/status");
    let reason_path = format!("/data/counterexample_provenance/values/{source_name}/reason");
    assert_eq!(
        diagnostic.pointer(&status_path).and_then(Value::as_str),
        Some("unraisable")
    );
    assert_eq!(
        diagnostic.pointer(&reason_path).and_then(Value::as_str),
        Some(ABSTRACT_CALL_RESULT_REASON)
    );
    assert_eq!(
        diagnostic.pointer("/data/counterexample_provenance/complete"),
        Some(&Value::Bool(false))
    );
}

fn verify_callsite_source(name: &str, source: &str) -> (PathBuf, Value) {
    let (dir, output, report) =
        verify_source(name, source, &["--json", "--enable-spurious-detection"]);
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "call-site precondition must fail: {text}"
    );
    assert_eq!(report["status"], "failed");
    assert!(
        report["reason"].as_str().is_some_and(|reason| {
            reason.contains("precondition (requires) not satisfied at call site")
        }),
        "{report}"
    );
    let diagnostic = lsp_diagnostic_for_source(name, source);
    (dir, diagnostic)
}

#[test]
fn ssa_counterexample_uses_the_pre_body_parameter_value() {
    // Before the snapshot fix, replay reused post-body x=10 and reported a spurious mismatch.
    let (dir, output, report) = verify_source(
        "ssa_bump",
        r#"
atom bump(x: i64) -> i64
requires: x >= 5;
ensures: result > 100;
body: { x = x * 2; x };
"#,
        &["--enable-spurious-detection"],
    );
    let text = output_text(&output);
    assert!(
        !output.status.success(),
        "the false ensures clause must fail: {text}"
    );
    assert!(!text.contains("Spurious"), "{text}");
    assert_eq!(report["counterexample"]["x"], "5");
    assert_eq!(
        report["counterexample_provenance"]["values"]["x"]["status"],
        "raised"
    );
    assert_eq!(
        report["semantic_feedback"]["counterexample_validation_status"],
        "validated"
    );
    assert_eq!(report["counterexample_fidelity"], "exact");
    cleanup(dir);
}

#[test]
fn struct_counterexample_and_reconstruction_loss_hide_solver_helpers() {
    // Previously p was the bare handle "0", while reconstruction loss exposed field helpers.
    let (dir, output, report) = verify_source(
        "struct_getp",
        r#"
struct Pt { a: i64, b: i64 }

atom getp(p: Pt) -> i64
requires: true;
ensures: result > 10;
body: p.a + p.b;
"#,
        &["--disable-spurious-detection"],
    );
    assert!(
        !output.status.success(),
        "the false ensures clause must fail"
    );
    let rendering = report["counterexample"]["p"].as_str().unwrap();
    assert!(rendering.starts_with("Pt { a: "), "{rendering}");
    assert!(rendering.contains(", b: "), "{rendering}");
    assert!(rendering.ends_with(" }"), "{rendering}");
    let loss = report["semantic_feedback"]["reconstruction_loss"]["counter_example"]
        .as_object()
        .expect("reconstruction loss counterexample");
    for key in report["counterexample"]
        .as_object()
        .unwrap()
        .keys()
        .chain(loss.keys())
    {
        assert!(
            !key.contains("__struct_")
                && !key.contains("__mumei_struct")
                && !key.contains("__proj_")
                && !key.starts_with("len_")
                && key != "p_a",
            "solver helper leaked as counterexample key {key}"
        );
    }
    cleanup(dir);
}

#[test]
fn callsite_struct_variable_argument_raises_fields() {
    // Before this fix, LSP data had p="0", lowering struct, status unraisable, reason "struct values require their binding fields".
    let source = format!(
        "{CHECK_CALLEE}\n{}",
        r#"
atom caller(q: Pt) -> i64
requires: true;
ensures: true;
body: check(q);
"#
    );
    let (dir, diagnostic) = verify_callsite_source("callsite_struct_variable", &source);
    let counterexample = diagnostic
        .pointer("/data/counterexample")
        .and_then(Value::as_object)
        .expect("call-site counterexample");
    let rendering = counterexample["p"].as_str().expect("struct argument value");
    assert!(rendering.starts_with("Pt { a: "), "{rendering}");
    assert!(rendering.contains(", b: "), "{rendering}");
    assert!(rendering.ends_with(" }"), "{rendering}");
    let a = rendering
        .strip_prefix("Pt { a: ")
        .and_then(|rest| rest.split_once(", b:").map(|(value, _)| value))
        .and_then(|value| value.parse::<i64>().ok())
        .expect("raised struct field a");
    assert!(a <= 0, "{rendering}");
    assert_eq!(
        diagnostic.pointer("/data/counterexample_provenance/values/p/status"),
        Some(&Value::String("raised".to_string()))
    );
    assert_eq!(
        diagnostic.pointer("/data/counterexample_provenance/values/p/lowering"),
        Some(&Value::String("struct".to_string()))
    );
    let value_keys = diagnostic
        .pointer("/data/counterexample_provenance/values")
        .and_then(Value::as_object)
        .expect("provenance values");
    for key in counterexample.keys().chain(value_keys.keys()) {
        assert!(
            !key.contains("__struct_") && !key.contains("__mumei_struct"),
            "solver helper leaked as source key {key}"
        );
    }
    cleanup(dir);
}

#[test]
fn callsite_struct_literal_argument_raises_fields() {
    let source = format!(
        "{CHECK_CALLEE}\n{}",
        r#"
atom caller_lit(x: i64) -> i64
requires: true;
ensures: true;
body: check(Pt { a: x, b: 1 });
"#
    );
    let (dir, diagnostic) = verify_callsite_source("callsite_struct_literal", &source);
    let rendering = diagnostic
        .pointer("/data/counterexample/p")
        .and_then(Value::as_str)
        .expect("raised struct argument");
    assert!(rendering.starts_with("Pt { a: "), "{rendering}");
    assert!(rendering.contains(", b: 1"), "{rendering}");
    let a = rendering
        .strip_prefix("Pt { a: ")
        .and_then(|rest| rest.split_once(", b:").map(|(value, _)| value))
        .and_then(|value| value.parse::<i64>().ok())
        .expect("raised struct field a");
    assert!(a <= 0, "{rendering}");
    assert_eq!(
        diagnostic.pointer("/data/counterexample_provenance/values/p/status"),
        Some(&Value::String("raised".to_string()))
    );
    cleanup(dir);
}

#[test]
fn callsite_struct_call_result_stays_unraisable() {
    let source = format!(
        "{CHECK_CALLEE}\n{}",
        r#"
atom mk(x: i64) -> Pt
requires: true;
ensures: true;
body: Pt { a: x, b: 0 };

atom caller_call(x: i64) -> i64
requires: true;
ensures: true;
body: check(mk(x));
"#
    );
    let (dir, diagnostic) = verify_callsite_source("callsite_struct_call_result", &source);
    assert_callsite_argument_unraisable(&diagnostic, "p");
    assert_eq!(
        diagnostic.pointer("/data/counterexample/p"),
        Some(&Value::String("0".to_string()))
    );
    cleanup(dir);
}

#[test]
fn callsite_struct_let_alias_of_call_result_stays_unraisable() {
    let source = format!(
        "{CHECK_CALLEE}\n{}",
        r#"
atom mk(x: i64) -> Pt
requires: true;
ensures: true;
body: Pt { a: x, b: 0 };

atom caller_call_alias(x: i64) -> i64
requires: true;
ensures: true;
body: {
    let r = mk(x);
    check(r)
};
"#
    );
    let (dir, diagnostic) = verify_callsite_source("callsite_struct_call_alias", &source);
    assert_callsite_argument_unraisable(&diagnostic, "p");
    assert_eq!(
        diagnostic.pointer("/data/counterexample/p"),
        Some(&Value::String("0".to_string()))
    );
    cleanup(dir);
}

#[test]
fn callsite_struct_result_field_prefix_does_not_match_other_bindings() {
    let source = r#"
struct Pt { a: i64, b: i64 }

atom check(p: Pt, p_x: Pt) -> i64
requires: p.a > 0;
ensures: true;
body: p.a;

atom mk(x: i64) -> Pt
requires: true;
ensures: true;
body: Pt { a: x, b: 0 };

atom caller2(p_x: Pt, q: Pt) -> i64
requires: true;
ensures: true;
body: {
    let p_r = mk(p_x.a);
    check(q, p_r)
};
"#
    .to_string();
    let (dir, diagnostic) = verify_callsite_source("callsite_struct_prefix_collision", &source);
    assert_eq!(
        diagnostic.pointer("/data/counterexample_provenance/values/p/status"),
        Some(&Value::String("raised".to_string()))
    );
    let rendering = diagnostic
        .pointer("/data/counterexample/p")
        .and_then(Value::as_str)
        .expect("raised struct argument");
    assert!(rendering.starts_with("Pt { a: "), "{rendering}");
    assert!(rendering.contains(", b: "), "{rendering}");
    cleanup(dir);
}

#[test]
fn callsite_scalar_direct_call_result_stays_unraisable() {
    let source = format!(
        "{MK_I_CALLEE}\n{CHECK_INT_CALLEE}\n{}",
        r#"
atom caller_direct(x: i64) -> i64
requires: true;
ensures: true;
body: check_int(mk_i(x));
"#
    );
    let (dir, diagnostic) = verify_callsite_source("callsite_scalar_direct_call_result", &source);
    assert_callsite_argument_unraisable(&diagnostic, "n");
    cleanup(dir);
}

#[test]
fn callsite_scalar_let_alias_of_call_result_stays_unraisable() {
    let source = format!(
        "{MK_I_CALLEE}\n{CHECK_INT_CALLEE}\n{}",
        r#"
atom caller_alias(x: i64) -> i64
requires: true;
ensures: true;
body: {
    let y = mk_i(x);
    check_int(y)
};
"#
    );
    let (dir, diagnostic) = verify_callsite_source("callsite_scalar_call_alias", &source);
    assert_callsite_argument_unraisable(&diagnostic, "n");
    cleanup(dir);
}

#[test]
fn callsite_struct_literal_scalar_call_field_stays_unraisable() {
    let source = format!(
        "{MK_I_CALLEE}\n{}",
        r#"
struct Pt { a: i64, b: i64 }

atom check(p: Pt) -> i64
requires: p.a > 0;
ensures: true;
body: p.a;

atom caller_struct_field(x: i64) -> i64
requires: true;
ensures: true;
body: check(Pt { a: mk_i(x), b: 1 });
"#
    );
    let (dir, diagnostic) =
        verify_callsite_source("callsite_struct_literal_scalar_call_field", &source);
    assert_callsite_argument_unraisable(&diagnostic, "p");
    cleanup(dir);
}

#[test]
fn callsite_scalar_source_argument_stays_raised() {
    let source = format!(
        "{CHECK_INT_CALLEE}\n{}",
        r#"
atom caller_source(x: i64) -> i64
requires: true;
ensures: true;
body: check_int(x);
"#
    );
    let (dir, diagnostic) = verify_callsite_source("callsite_scalar_source_argument", &source);
    assert_eq!(
        diagnostic.pointer("/data/counterexample_provenance/values/n/status"),
        Some(&Value::String("raised".to_string()))
    );
    cleanup(dir);
}

#[test]
fn bitvec_i64_counterexample_is_signed_decimal() {
    // Before raising, the same value was printed as a hexadecimal #x bit-vector.
    let (dir, output, report) = verify_source(
        "bitvec",
        r#"
atom negative(x: i64) -> i64
requires: true;
ensures: result >= 0;
body: x;
"#,
        &["--bitvec-i64", "--disable-spurious-detection"],
    );
    assert!(!output.status.success(), "the negative model must fail");
    let value = report["counterexample"]["x"].as_str().unwrap();
    assert!(value.parse::<i64>().unwrap() < 0, "{value}");
    assert!(!value.contains("#x"), "{value}");
    assert_eq!(
        report["counterexample_provenance"]["values"]["x"]["lowering"],
        "bitvec_i64"
    );
    cleanup(dir);
}

#[test]
fn default_and_ieee_f64_counterexamples_use_source_renderings() {
    // Before raising, exact Real and IEEE model syntax leaked as (/ ...) and (fp ...).
    let source = r#"
atom fraction(x: f64) -> f64
requires: x == 0.5;
ensures: result > 0.75;
body: x;
"#;
    for (name, args) in [
        ("real_f64", vec!["--disable-spurious-detection"]),
        (
            "ieee_f64",
            vec!["--ieee754-f64", "--disable-spurious-detection"],
        ),
    ] {
        let (dir, output, report) = verify_source(name, source, &args);
        assert!(
            !output.status.success(),
            "the false ensures clause must fail"
        );
        let value = report["counterexample"]["x"].as_str().unwrap();
        assert_eq!(value.parse::<f64>().unwrap(), 0.5, "{value}");
        assert!(!value.contains("(/"), "{value}");
        assert!(!value.contains("(fp"), "{value}");
        assert_eq!(
            report["counterexample_provenance"]["values"]["x"]["status"],
            "raised"
        );
        cleanup(dir);
    }
}

#[test]
fn enum_counterexample_uses_source_constructor_syntax() {
    // Before raising, a datatype witness was printed as the solver form "(Sq 0)".
    let (dir, output, report) = verify_source(
        "enum_shape",
        r#"
enum Shape { Circle(i64), Sq(i64) }

atom area(s: Shape) -> i64
requires: s == Shape::Sq(0);
ensures: result > 10;
body: match s { Circle(n) => n, Sq(n) => n };
"#,
        &["--disable-spurious-detection"],
    );
    assert!(
        !output.status.success(),
        "the false ensures clause must fail"
    );
    let value = report["counterexample"]["s"].as_str().unwrap();
    assert!(value.starts_with("Shape::Sq("), "{value}");
    assert!(!value.starts_with("(Sq"), "{value}");
    assert_eq!(
        report["counterexample_provenance"]["values"]["s"]["status"],
        "raised"
    );
    cleanup(dir);
}

#[test]
fn string_length_companion_is_raised_in_loss_and_provenance() {
    let dir = std::env::temp_dir().join(format!(
        "mumei_string_length_provenance_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create report directory");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--json")
        .arg("--enable-spurious-detection")
        .arg("--report-dir")
        .arg(&dir)
        .arg("tests/negative/test_str_len_cex.mm")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run string length counterexample fixture");
    assert!(
        !output.status.success(),
        "fixture must fail verification: {}",
        output_text(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let payload: Value =
        serde_json::from_str(stdout.trim()).expect("verify --json emits one JSON payload");
    let provenance = &payload["counterexample_provenance"];
    assert_eq!(provenance["values"]["len_s"]["status"], "raised");
    assert_eq!(provenance["values"]["len_s"]["lowering"], "str_length");
    assert!(
        !provenance["omitted_solver_symbols"]
            .as_array()
            .expect("provenance contains omitted solver symbols")
            .iter()
            .any(|symbol| symbol == "len_s"),
        "a raised length companion must not also be omitted"
    );

    let counterexample = &payload["counterexample"];
    assert!(counterexample.is_object());
    assert!(
        counterexample.get("len_s").is_none(),
        "length companions are reconstruction-loss values, not public counterexample fields"
    );
    let loss = &payload["semantic_feedback"]["reconstruction_loss"]["counter_example"];
    let string = loss["s"]
        .as_str()
        .expect("string parameter is present in reconstruction loss");
    let len_s = loss["len_s"]
        .as_i64()
        .expect("raised string length is an integer");
    assert_eq!(len_s, string.len() as i64);
    cleanup(dir);
}

#[test]
fn non_ascii_string_loss_is_decoded_with_byte_length() {
    // On develop at d5fe4b6, loss `s` was `\u{e6}\u{97}\u{a5}\u{e6}\u{9c}\u{ac}` and `len_s` was 6.
    let dir = std::env::temp_dir().join(format!(
        "mumei_non_ascii_string_loss_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create report directory");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--json")
        .arg("--enable-spurious-detection")
        .arg("--report-dir")
        .arg(&dir)
        .arg("tests/negative/test_str_non_ascii_cex.mm")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run non-ASCII string counterexample fixture");
    assert!(
        !output.status.success(),
        "fixture must fail verification: {}",
        output_text(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let payload: Value =
        serde_json::from_str(stdout.trim()).expect("verify --json emits one JSON payload");
    let provenance = &payload["counterexample_provenance"]["values"]["s"];
    assert_eq!(provenance["status"], "raised");
    assert!(payload["counterexample"]["s"]
        .as_str()
        .expect("public counterexample contains string rendering")
        .contains(r"\u{e6}\u{97}\u{a5}\u{e6}\u{9c}\u{ac}"));
    let loss = &payload["semantic_feedback"]["reconstruction_loss"]["counter_example"];
    assert_eq!(loss["s"], "日本");
    // len_s is the byte length, matching the solver lowering, runtime strlen, and develop.
    assert_eq!(loss["len_s"], 6);
    cleanup(dir);
}

#[test]
fn array_parameter_is_retained_even_when_it_cannot_be_raised() {
    // Previously array parameters could disappear into raw array and length symbols.
    let (dir, output, report) = verify_source(
        "array_param",
        r#"
atom use_array(arr: [i64], n: i64) -> i64
requires: n > 0;
ensures: result > n;
body: n;
"#,
        &["--disable-spurious-detection"],
    );
    assert!(
        !output.status.success(),
        "the false ensures clause must fail"
    );
    assert!(report["counterexample"]["arr"].is_string());
    let provenance = &report["counterexample_provenance"]["values"]["arr"];
    assert!(
        provenance["status"] == "raised"
            || (provenance["status"] == "unraisable" && provenance["reason"].is_string()),
        "{provenance}"
    );
    let length_provenance = &report["counterexample_provenance"]["values"]["len_arr"];
    assert_eq!(length_provenance["status"], "raised");
    assert_eq!(length_provenance["lowering"], "array_length");
    assert!(
        !report["counterexample_provenance"]["omitted_solver_symbols"]
            .as_array()
            .unwrap()
            .iter()
            .any(|symbol| symbol == "len_arr")
    );
    assert!(
        report["semantic_feedback"]["reconstruction_loss"]["counter_example"]["len_arr"]
            .as_i64()
            .is_some()
    );
    if provenance["status"] == "unraisable" {
        assert_eq!(report["counterexample_fidelity"], "approximate");
    }
    cleanup(dir);
}

#[test]
fn bitvec_array_length_companion_is_raised() {
    let (dir, output, report) = verify_source(
        "bitvec_array_length",
        r#"
atom array_length_failure(arr: [i64], n: i64) -> i64
requires: n > 0 && len(arr) == n;
ensures: result > n;
body: n;
"#,
        &["--bitvec-i64", "--disable-spurious-detection"],
    );
    assert!(
        !output.status.success(),
        "the false ensures clause must fail"
    );
    let length = &report["counterexample_provenance"]["values"]["len_arr"];
    assert_eq!(length["status"], "raised");
    assert_eq!(length["lowering"], "array_length");
    assert!(
        report["semantic_feedback"]["reconstruction_loss"]["counter_example"]["len_arr"]
            .as_i64()
            .is_some()
    );
    cleanup(dir);
}

#[test]
fn refinement_counterexample_retains_the_declared_type() {
    // Previously the value was only a base integer, with no refinement type provenance.
    let (dir, output, report) = verify_source(
        "refinement",
        r#"
type Nat = i64 where v >= 0;

atom too_small(x: Nat) -> i64
requires: true;
ensures: result > 10;
body: x;
"#,
        &["--disable-spurious-detection"],
    );
    assert!(
        !output.status.success(),
        "the false ensures clause must fail"
    );
    assert_eq!(report["counterexample"]["x"], "0");
    assert_eq!(
        report["counterexample_provenance"]["values"]["x"]["source_type"],
        "Nat"
    );
    cleanup(dir);
}

#[test]
fn u64_counterexample_keeps_integer_provenance_and_replays_exactly() {
    // Before typed raising, u64 model values were unraisable and reduced fidelity.
    let (dir, output, report) = verify_source(
        "u64_counterexample",
        r#"
atom u64_counterexample(x: u64) -> u64
requires: x == 7;
ensures: result > 10;
body: x;
"#,
        &["--enable-spurious-detection"],
    );
    assert!(
        !output.status.success(),
        "the false ensures clause must fail: {}",
        output_text(&output)
    );
    assert_eq!(report["counterexample"]["x"], "7");
    assert_eq!(
        report["counterexample_provenance"]["values"]["x"]["status"],
        "raised"
    );
    assert_eq!(
        report["counterexample_provenance"]["values"]["x"]["lowering"],
        "int"
    );
    let reconstruction_loss = &report["semantic_feedback"]["reconstruction_loss"];
    assert_eq!(reconstruction_loss["counter_example"]["x"], 7);
    let x_component = reconstruction_loss["loss_components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|component| component["variable"] == "x")
        .expect("x loss component");
    assert_eq!(x_component["observed"], 7);
    assert_eq!(x_component["magnitude"], 7.0);
    assert_eq!(
        report["semantic_feedback"]["counterexample_validation_status"],
        "validated"
    );
    assert_eq!(report["counterexample_fidelity"], "exact");
    cleanup(dir);
}

#[test]
fn struct_body_local_is_not_reported_as_its_integer_handle() {
    // Previously the local struct handle was raised as an integer and reported as "0".
    let (dir, output, report) = verify_source(
        "struct_body_local",
        r#"
struct Pt { a: i64, b: i64 }

atom mk(x: i64) -> i64
requires: x >= 0;
ensures: result > 100;
body: {
    let q = Pt { a: x, b: 1 };
    q.a
};
"#,
        &["--disable-spurious-detection"],
    );
    assert!(
        !output.status.success(),
        "the false ensures clause must fail: {}",
        output_text(&output)
    );

    let loss = &report["semantic_feedback"]["reconstruction_loss"]["counter_example"];
    let q_rendering = loss["q"].as_str().expect("structured q rendering");
    let q_provenance = &report["counterexample_provenance"]["values"]["q"];
    assert_ne!(q_provenance["lowering"], "int");
    match q_provenance["status"].as_str() {
        Some("raised") => {
            assert_eq!(q_provenance["lowering"], "struct");
            assert_eq!(q_provenance["source_type"], "Pt");
            assert_ne!(q_rendering, "0");
            assert!(q_rendering.starts_with("Pt { a: "), "{q_rendering}");
            assert!(q_rendering.contains(", b: 1"), "{q_rendering}");
        }
        Some("unraisable") => {}
        status => panic!("expected raised or unraisable q provenance, got {status:?}"),
    }
    cleanup(dir);
}
