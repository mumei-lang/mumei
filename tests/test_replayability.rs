//! Replayability of non-deterministic effects (Random / Clock / ExternalInput).
//!
//! * The witness parameter (seed / timestamp / input) is mandatory and must be
//!   threaded into every `perform` of the effect.
//! * Pure atoms performing a non-deterministic source stay rejected by effect
//!   containment.
//! * Property-based validation replays the same trace for a fixed seed.

use std::fs;
use std::process::Command;

use mumei_core::parser::{parse_module, Item};
use mumei_core::verification::{run_property_based_test, ModuleEnv, PropertyBasedTestConfig};

fn mumei_verify(file: &str) -> (bool, String) {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let output = Command::new(bin)
        .arg("verify")
        .arg(file)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei verify {file}: {err}"));
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), combined)
}

/// Verify a copy in a fresh directory so the verification cache cannot skip atoms.
fn mumei_verify_uncached(file: &str) -> (bool, String) {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
    let dir = std::env::temp_dir().join(format!("mumei_replay_{}", std::process::id()));
    fs::create_dir_all(&dir).expect("fixture dir");
    let dst = dir.join("main.mm");
    fs::copy(&src, &dst).expect("copy fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(&dst)
        .current_dir(&dir)
        .output()
        .expect("run mumei verify");
    let _ = fs::remove_dir_all(&dir);
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), combined)
}

#[test]
fn seeded_nondeterministic_atoms_verify() {
    let (ok, out) = mumei_verify_uncached("tests/test_replay_random_seeded.mm");
    assert!(
        ok,
        "seeded Random/Clock/ExternalInput atoms must verify:\n{out}"
    );
    assert!(
        out.contains("'replay_twice': verified"),
        "two performs with the same witness must denote the same value:\n{out}"
    );
    for atom in ["roll_derived", "roll_branch"] {
        assert!(
            out.contains(&format!("'{atom}': verified")),
            "derived witness must be accepted for {atom}:\n{out}"
        );
    }
}

#[test]
fn missing_witness_parameter_is_rejected() {
    let (ok, out) = mumei_verify("tests/negative/test_replay_missing_witness.mm");
    assert!(!ok, "Random without a seed parameter must fail:\n{out}");
    assert!(
        out.contains("Replayability violation") && out.contains("seed: i64"),
        "expected replayability diagnostic with witness suggestion:\n{out}"
    );
}

#[test]
fn witness_not_threaded_into_perform_is_rejected() {
    let (ok, out) = mumei_verify("tests/negative/test_replay_witness_not_threaded.mm");
    assert!(
        !ok,
        "perform Random.next(x) must fail when seed is unused:\n{out}"
    );
    assert!(
        out.contains("without threading its witness parameter"),
        "expected threading diagnostic:\n{out}"
    );
}

#[test]
fn perform_nested_in_array_literal_is_still_checked() {
    let (ok, out) = mumei_verify("tests/negative/test_replay_nested_perform.mm");
    assert!(!ok, "nested perform Random.next(42) must fail:\n{out}");
    assert!(
        out.contains("without threading its witness parameter"),
        "expected threading diagnostic:\n{out}"
    );
}

#[test]
fn lambda_parameter_shadowing_witness_is_rejected() {
    let (ok, out) = mumei_verify("tests/negative/test_replay_lambda_shadow.mm");
    assert!(
        !ok,
        "lambda-local seed must not count as the witness:\n{out}"
    );
    assert!(
        out.contains("without threading its witness parameter"),
        "expected threading diagnostic:\n{out}"
    );
}

#[test]
fn witness_read_only_in_condition_is_rejected() {
    let (ok, out) = mumei_verify("tests/negative/test_replay_guard_only.mm");
    assert!(
        !ok,
        "`if seed > 0 {{ x }} else {{ x }}` is not seed-derived:\n{out}"
    );
    assert!(
        out.contains("without threading its witness parameter"),
        "expected threading diagnostic:\n{out}"
    );
}

#[test]
fn binding_inside_uncalled_lambda_does_not_leak_witness() {
    let (ok, out) = mumei_verify("tests/negative/test_replay_lambda_leak.mm");
    assert!(
        !ok,
        "lambda-internal `s = seed` must not witness the outer perform:\n{out}"
    );
    assert!(
        out.contains("without threading its witness parameter"),
        "expected threading diagnostic:\n{out}"
    );
}

#[test]
fn pure_atom_performing_nondeterministic_source_is_rejected_by_containment() {
    let (ok, out) = mumei_verify("tests/negative/test_replay_pure_atom.mm");
    assert!(!ok, "pure atom performing Random must fail:\n{out}");
    assert!(
        out.contains("Add 'Random' to the effects declaration"),
        "expected effect containment diagnostic, not a replayability one:\n{out}"
    );
    assert!(!out.contains("Replayability violation"), "{out}");
}

fn seeded_source(ensures: &str) -> String {
    seeded_source_with_body(
        ensures,
        "let r = perform Random.next(seed);\n    if r >= 0 { r } else { 0 - r }",
    )
}

fn seeded_source_with_body(ensures: &str, body: &str) -> String {
    format!(
        "effect Random;\natom roll(seed: i64) -> i64\neffects: [Random];\nensures: {ensures};\n\
         body: {{\n    {body}\n}};\n"
    )
}

fn property_result(source: &str, seed: u64) -> mumei_core::verification::PropertyBasedTestResult {
    let items = parse_module(source);
    let mut module_env = ModuleEnv::new();
    let mut atom = None;
    for item in items {
        match item {
            Item::EffectDef(def) => {
                module_env.effect_defs.insert(def.name.clone(), def);
            }
            Item::Atom(a) => atom = Some(a),
            _ => {}
        }
    }
    let atom = atom.expect("fixture atom");
    assert_eq!(atom.params.len(), 1, "{atom:?}");
    let config = PropertyBasedTestConfig {
        test_count: 32,
        max_shrink_steps: 16,
        seed,
        include_boundary_values: true,
    };
    run_property_based_test(&atom, &module_env, &config)
}

#[test]
fn property_based_validation_replays_seeded_nondeterministic_atom() {
    let source = seeded_source("result >= 0");
    let first = property_result(&source, 0x5EED);
    let second = property_result(&source, 0x5EED);
    assert!(first.tests_run > 0, "{first:?}");
    assert_eq!(first.status, "passed", "{first:?}");
    assert_eq!(first, second, "fixed seed must replay identically");
}

#[test]
fn property_based_counterexample_for_nondeterministic_atom_is_reproducible() {
    let source = seeded_source("result == 0");
    let first = property_result(&source, 0x5EED);
    let second = property_result(&source, 0x5EED);
    assert_eq!(first.status, "failed", "{first:?}");
    assert!(first.shrunk_counterexample.is_some(), "{first:?}");
    assert_eq!(
        first, second,
        "fixed seed must replay the same counterexample"
    );
}

/// Two performs with different arguments are both pinned: `a - a2` with equal
/// arguments is always 0, while `a - b` with different arguments is not.
#[test]
fn property_based_pins_every_perform_by_its_arguments() {
    let same = seeded_source_with_body(
        "result == 0",
        "let a = perform Random.next(seed);\n    let a2 = perform Random.next(seed);\n    a - a2",
    );
    let same_result = property_result(&same, 0x5EED);
    assert_eq!(same_result.status, "passed", "{same_result:?}");

    let different = seeded_source_with_body(
        "result == 0",
        "let a = perform Random.next(seed);\n    let b = perform Random.next(seed + 1);\n    a - b",
    );
    let first = property_result(&different, 0x5EED);
    let second = property_result(&different, 0x5EED);
    assert_eq!(first.status, "failed", "{first:?}");
    assert_eq!(
        first, second,
        "fixed seed must replay the same counterexample"
    );
}
