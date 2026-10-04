use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

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
    if provenance["status"] == "unraisable" {
        assert_eq!(report["counterexample_fidelity"], "approximate");
    }
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
