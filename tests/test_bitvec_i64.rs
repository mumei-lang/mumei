//! Integration tests for the opt-in `--bitvec-i64` verification mode (P10-A).
//!
//! Default `i64` verification encodes values as Z3 `Int` (unbounded integers).
//! With `--bitvec-i64` (or per-atom, when a contract uses a bitwise operator or
//! declares `semantics: bitvec;`), `i64` is encoded as `BV(64)`: bitwise
//! operators get real bit semantics and `+`/`-`/`*` wrap like machine
//! arithmetic. These tests pin the differential, check `std/bitwise.mm` proves
//! its real bit-level `ensures`, and gate backward compatibility of default
//! mode against a committed certificate golden file.

use std::process::Command;

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mumei_bitvec_{}_{}", std::process::id(), tag));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale temp dir");
    }
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_fixture(dir: &std::path::Path, name: &str, source: &str) -> std::path::PathBuf {
    let fixture = dir.join(name);
    std::fs::write(&fixture, source).expect("write fixture");
    fixture
}

fn run_verify(
    fixture: &std::path::Path,
    dir: &std::path::Path,
    extra: &[&str],
) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let mut cmd = Command::new(bin);
    cmd.arg("verify");
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.arg("--report-dir")
        .arg(dir)
        .arg(fixture)
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    cmd.output().expect("failed to run mumei verify")
}

fn assert_verified(output: &std::process::Output, what: &str) {
    assert!(
        output.status.success(),
        "expected {what} to verify\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_rejected(output: &std::process::Output, what: &str) {
    assert!(
        !output.status.success(),
        "expected {what} to be rejected\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Concrete bit patterns: `&`, `|`, `^`, `<<` and the sign-propagating `>>`
/// must evaluate to their machine values, not to an uninterpreted stub.
#[test]
fn bitvec_proves_concrete_bit_patterns() {
    let dir = temp_dir("patterns");
    let source = "atom patterns() -> i64\n\
        requires: true;\n\
        ensures: (12 & 10) == 8 && (12 | 10) == 14 && (12 ^ 10) == 6\n\
              && (1 << 3) == 8 && (0 - 8) >> 1 == 0 - 4 && (0 - 1) >> 63 == 0 - 1;\n\
        body: 0;\n";
    let fixture = write_fixture(&dir, "patterns.mm", source);

    assert_verified(
        &run_verify(&fixture, &dir, &["--bitvec-i64"]),
        "concrete bit patterns under --bitvec-i64",
    );
    // Bitwise operators pull in the bit-vector encoding per atom even without
    // the flag, because `Int` cannot express them.
    assert_verified(
        &run_verify(&fixture, &dir, &[]),
        "concrete bit patterns without the flag",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// A wrong bit-level claim must produce a counterexample rather than pass
/// through a vacuous witness.
#[test]
fn bitvec_rejects_wrong_bit_claim() {
    let dir = temp_dir("wrong");
    let source = "atom wrong(a: i64, b: i64) -> i64\n\
        requires: true;\n\
        ensures: result == a & b;\n\
        body: a | b;\n";
    let fixture = write_fixture(&dir, "wrong.mm", source);

    assert_rejected(
        &run_verify(&fixture, &dir, &["--bitvec-i64"]),
        "`result == a & b` for a body computing `a | b`",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// `i64` arithmetic wraps under `--bitvec-i64` (two's complement) but is
/// unbounded in the default `Int` mode. The same source must therefore verify
/// only with the flag, which is the whole point of making the mode opt-in.
#[test]
fn bitvec_flag_makes_i64_arithmetic_wrap() {
    let dir = temp_dir("wrap");
    let source = "atom wrap() -> i64\n\
        requires: true;\n\
        ensures: 9223372036854775807 + 1 < 0;\n\
        body: 0;\n";
    let fixture = write_fixture(&dir, "wrap.mm", source);

    assert_verified(
        &run_verify(&fixture, &dir, &["--bitvec-i64"]),
        "wrapping `i64::MAX + 1 < 0` under --bitvec-i64",
    );
    assert_rejected(
        &run_verify(&fixture, &dir, &[]),
        "wrapping claim in default unbounded `Int` mode",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// `semantics: bitvec;` selects the bit-vector encoding for an atom whose
/// contract depends on wrapping without naming a bitwise operator.
#[test]
fn semantics_bitvec_clause_selects_bitvector_encoding() {
    let dir = temp_dir("clause");
    let source = "atom wrap_clause() -> i64\n\
        semantics: bitvec;\n\
        requires: true;\n\
        ensures: 9223372036854775807 + 1 < 0;\n\
        body: 0;\n";
    let fixture = write_fixture(&dir, "clause.mm", source);

    assert_verified(
        &run_verify(&fixture, &dir, &[]),
        "`semantics: bitvec;` atom without the CLI flag",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// Shifts are only specified for `0 <= n < 64`; an unguarded shift amount must
/// not be provable.
#[test]
fn bitvec_requires_bounded_shift_amount() {
    let dir = temp_dir("shift");
    let guarded = write_fixture(
        &dir,
        "guarded.mm",
        "atom guarded(x: i64, n: i64) -> i64\n\
         requires: n >= 0 && n < 64;\n\
         ensures: result == x << n;\n\
         body: x << n;\n",
    );
    let unguarded = write_fixture(
        &dir,
        "unguarded.mm",
        "atom unguarded(x: i64, n: i64) -> i64\n\
         requires: true;\n\
         ensures: result == x << n && n >= 0;\n\
         body: x << n;\n",
    );

    assert_verified(
        &run_verify(&guarded, &dir, &["--bitvec-i64"]),
        "shift with `0 <= n < 64` precondition",
    );
    assert_rejected(
        &run_verify(&unguarded, &dir, &["--bitvec-i64"]),
        "unguarded shift claiming `n >= 0`",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// Success criterion: every `std/bitwise.mm` atom proves its real bit-level
/// `ensures`, both in default mode (per-atom bit-vector encoding) and with the
/// flag set globally.
#[test]
fn stdlib_bitwise_atoms_verify_with_real_bit_semantics() {
    let dir = temp_dir("stdlib");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("std/bitwise.mm");

    for extra in [&[][..], &["--bitvec-i64"][..]] {
        let output = run_verify(&fixture, &dir, extra);
        assert_verified(&output, "std/bitwise.mm");
        let stdout = String::from_utf8_lossy(&output.stdout);
        for atom in [
            "bit_and",
            "bit_or",
            "bit_xor",
            "bit_shift_left",
            "bit_shift_right",
        ] {
            assert!(
                stdout.contains(atom),
                "expected `{atom}` in verify output (args: {extra:?}):\n{stdout}"
            );
        }
    }

    std::fs::remove_dir_all(dir).ok();
}

/// A bitwise operator that only appears in a loop invariant still selects the
/// bit-vector encoding for the atom.
#[test]
fn bitwise_in_invariant_selects_bitvector_encoding() {
    let dir = temp_dir("invariant");
    let source = "atom masked_loop(n: i64) -> i64\n\
        requires: n >= 0 && n < 8;\n\
        ensures: result >= 0;\n\
        body: {\n\
            let i = 0;\n\
            while i < n\n\
            invariant: i >= 0 && i <= n && (i & 7) == i\n\
            decreases: n - i\n\
            {\n\
                i = i + 1;\n\
            };\n\
            i\n\
        };\n";
    let fixture = write_fixture(&dir, "invariant.mm", source);

    assert_verified(
        &run_verify(&fixture, &dir, &[]),
        "atom whose only bitwise operation is in a loop invariant",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// A bitwise operator confined to the condition of a `forall` extracted from
/// `requires` also selects the bit-vector encoding.
#[test]
fn bitwise_in_forall_condition_selects_bitvector_encoding() {
    let dir = temp_dir("forall");
    let source = "atom masked_elements(n: i64) -> i64\n\
        requires: n >= 0 && forall(i, 0, n, (arr[i] & 1) == 0);\n\
        ensures: result == n;\n\
        body: n;\n";
    let fixture = write_fixture(&dir, "forall.mm", source);

    assert_verified(
        &run_verify(&fixture, &dir, &[]),
        "atom whose only bitwise operation is in a forall condition",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// Array indices stay `Int`-sorted under the global flag: an `i64` index
/// parameter is bridged from `BV(64)` at the access boundary.
#[test]
fn bitvec_mode_supports_array_indexing_with_i64_index() {
    let dir = temp_dir("array");
    let source = "atom read_at(i: i64, n: i64) -> i64\n\
        requires: n >= 0 && i >= 0 && i < n && len(arr) == n && arr[i] >= 0;\n\
        ensures: result >= 0;\n\
        body: arr[i];\n";
    let fixture = write_fixture(&dir, "array.mm", source);

    assert_verified(
        &run_verify(&fixture, &dir, &["--bitvec-i64"]),
        "`arr[i]` with an i64 index under --bitvec-i64",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// Shift-range validation also covers shifts that only occur in contract
/// clauses, which are lowered without a solver.
#[test]
fn contract_only_shift_needs_a_bounded_amount() {
    let dir = temp_dir("clause_shift");
    let unguarded = write_fixture(
        &dir,
        "clause_shift.mm",
        "atom clause_shift(x: i64, n: i64) -> i64\n\
         requires: (x << n) >= 0;\n\
         ensures: result == x;\n\
         body: x;\n",
    );
    let guarded = write_fixture(
        &dir,
        "clause_shift_ok.mm",
        "atom clause_shift_ok(x: i64, n: i64) -> i64\n\
         requires: n >= 0 && n < 64 && (x << n) >= 0;\n\
         ensures: result == x;\n\
         body: x;\n",
    );

    assert_rejected(
        &run_verify(&unguarded, &dir, &["--bitvec-i64"]),
        "shift in `requires` without a bound on the amount",
    );
    assert_verified(
        &run_verify(&guarded, &dir, &["--bitvec-i64"]),
        "shift in `requires` with `0 <= n < 64`",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// A bit-vector caller must not import the `ensures` of a callee that was
/// proved in the `Int` encoding: `result > x` for `x + 1` is true for unbounded
/// integers and false at `i64::MAX` under wrapping. The same call chain without
/// a mode boundary keeps using the callee's contract.
#[test]
fn callee_contract_is_not_imported_across_a_mode_boundary() {
    let dir = temp_dir("boundary");
    let callee = "atom bump(x: i64) -> i64\n\
                  requires: true;\n\
                  ensures: result > x;\n\
                  body: x + 1;\n\n";
    let crossing = write_fixture(
        &dir,
        "crossing.mm",
        &format!(
            "{callee}atom bump_bits(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result > (x | 0);\n\
             body: bump(x | 0);\n"
        ),
    );
    let same_mode = write_fixture(
        &dir,
        "same_mode.mm",
        &format!(
            "{callee}atom bump_twice(x: i64) -> i64\n\
             requires: true;\n\
             ensures: result > x;\n\
             body: bump(x);\n"
        ),
    );

    assert_rejected(
        &run_verify(&crossing, &dir, &[]),
        "an `Int`-mode `ensures` assumed by a bit-vector caller",
    );
    assert_verified(
        &run_verify(&same_mode, &dir, &[]),
        "a call chain that stays in the `Int` encoding",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// Branches of an `if`/`match` may mix a `BV(64)` value with an integer
/// literal; both sides are bridged to one sort instead of building an `ite`
/// over mixed sorts.
#[test]
fn bitvec_branches_accept_a_literal_arm() {
    let dir = temp_dir("branches");
    let auto = write_fixture(
        &dir,
        "branches.mm",
        "atom masked_or_five(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result >= 0;\n\
         body: { if (x & 1) == 0 { x & 1 } else { 5 } };\n",
    );
    let flagged = write_fixture(
        &dir,
        "branches_flag.mm",
        "atom pos_or_five(x: i64) -> i64\n\
         requires: x >= 0;\n\
         ensures: result >= 0;\n\
         body: { if x >= 0 { x } else { 5 } };\n",
    );

    assert_verified(
        &run_verify(&auto, &dir, &[]),
        "bit-vector branch against a literal branch",
    );
    assert_verified(
        &run_verify(&flagged, &dir, &["--bitvec-i64"]),
        "bit-vector branch against a literal branch under --bitvec-i64",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// A `forall` whose bound is a `BV(64)` parameter keeps its facts: the bound is
/// bridged to `Int`, not replaced by a fresh unconstrained constant that would
/// make the quantifier vacuous.
#[test]
fn forall_over_a_bitvector_bound_is_not_vacuous() {
    let dir = temp_dir("forall_bound");
    let provable = write_fixture(
        &dir,
        "forall_bound.mm",
        "atom first_is_zero(n: i64) -> i64\n\
         requires: n == 3 && len(arr) == n && forall(i, 0, n, arr[i] == 0);\n\
         ensures: arr[0] == 0;\n\
         body: 0;\n",
    );
    let false_claim = write_fixture(
        &dir,
        "forall_bound_false.mm",
        "atom first_is_one(n: i64) -> i64\n\
         requires: n == 3 && len(arr) == n && forall(i, 0, n, arr[i] == 0);\n\
         ensures: arr[0] == 1;\n\
         body: 0;\n",
    );

    assert_verified(
        &run_verify(&provable, &dir, &["--bitvec-i64"]),
        "forall fact over a BV(64) bound",
    );
    assert_rejected(
        &run_verify(&false_claim, &dir, &["--bitvec-i64"]),
        "postcondition contradicting the forall fact",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// A refinement alias over `i64` keeps the active encoding, so wrapping is
/// still visible through the alias.
#[test]
fn refined_i64_keeps_the_bitvector_encoding() {
    let dir = temp_dir("refined");
    let source = "type NonNegative = i64 where v >= 0;\n\
        atom wraps(x: NonNegative) -> i64\n\
        requires: x >= 9223372036854775807;\n\
        ensures: result < 0;\n\
        body: x + 1;\n";
    let fixture = write_fixture(&dir, "refined.mm", source);

    assert_verified(
        &run_verify(&fixture, &dir, &["--bitvec-i64"]),
        "wrapping increment of a refined i64 parameter",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// An atom-level `invariant` is verified in its own context, which still has to
/// discharge the shift-range obligations collected while lowering it.
#[test]
fn atom_invariant_shift_needs_a_bounded_amount() {
    let dir = temp_dir("inv_shift");
    let unguarded = write_fixture(
        &dir,
        "inv_shift.mm",
        "atom inv_shift(x: i64, n: i64) -> i64\n\
         requires: x >= 0;\n\
         invariant: (x << n) == (x << n);\n\
         ensures: result == x;\n\
         body: x;\n",
    );
    let guarded = write_fixture(
        &dir,
        "inv_shift_ok.mm",
        "atom inv_shift_ok(x: i64, n: i64) -> i64\n\
         requires: x >= 0 && n >= 0 && n < 64;\n\
         invariant: (x << n) == (x << n);\n\
         ensures: result == x;\n\
         body: x;\n",
    );

    assert_rejected(
        &run_verify(&unguarded, &dir, &["--bitvec-i64"]),
        "atom invariant shifting by an unbounded amount",
    );
    assert_verified(
        &run_verify(&guarded, &dir, &["--bitvec-i64"]),
        "atom invariant shifting by a bounded amount",
    );

    std::fs::remove_dir_all(dir).ok();
}

/// `mumei build` puts the effective semantic mode into the incremental cache
/// key, so declaring `semantics: bitvec;` re-verifies an otherwise unchanged
/// atom instead of reusing the `Int`-mode proof.
#[test]
fn build_cache_key_tracks_the_semantic_mode() {
    let dir = temp_dir("build_cache");
    let int_source = "atom inc(x: i64) -> i64\n\
        requires: x >= 0 && x < 10;\n\
        ensures: result == x + 1;\n\
        body: x + 1;\n";
    let fixture = write_fixture(&dir, "cache.mm", int_source);

    let run_build = || {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .arg("build")
            .arg(&fixture)
            .arg("--output")
            .arg(dir.join("out"))
            .current_dir(&dir)
            .output()
            .expect("failed to run mumei build")
    };

    assert_verified(&run_build(), "first build");
    let cached = run_build();
    assert_verified(&cached, "second build of an unchanged atom");
    assert!(
        String::from_utf8_lossy(&cached.stdout).contains("unchanged, cached"),
        "an unchanged atom must hit the build cache:\n{}",
        String::from_utf8_lossy(&cached.stdout)
    );

    write_fixture(
        &dir,
        "cache.mm",
        "atom inc(x: i64) -> i64\n\
         semantics: bitvec;\n\
         requires: x >= 0 && x < 10;\n\
         ensures: result == x + 1;\n\
         body: x + 1;\n",
    );
    let switched = run_build();
    assert_verified(&switched, "build after declaring `semantics: bitvec;`");
    assert!(
        !String::from_utf8_lossy(&switched.stdout).contains("unchanged, cached"),
        "switching to bit-vector semantics must invalidate the build cache:\n{}",
        String::from_utf8_lossy(&switched.stdout)
    );

    std::fs::remove_dir_all(dir).ok();
}

// ---------------------------------------------------------------------------
// Backward compatibility gate: zero proof-certificate regressions with the
// flag off.
// ---------------------------------------------------------------------------

/// Certificate fields of an atom that must not move when the flag is off.
fn certificate_summary(cert: &serde_json::Value) -> serde_json::Value {
    let atoms: Vec<serde_json::Value> = cert["atoms"]
        .as_array()
        .expect("certificate atoms array")
        .iter()
        .map(|atom| {
            serde_json::json!({
                "name": atom["name"],
                "status": atom["status"],
                "z3_check_result": atom["z3_check_result"],
                "content_hash": atom["content_hash"],
                "proof_hash": atom["proof_hash"],
                "logic_fragment_tag": atom["logic_fragment_tag"],
                "logic_fragment_tags": atom["logic_fragment_tags"],
                "lowering_rules": atom["translator_ir"]["lowering_rules"],
                "binder_lean_types": atom["translator_ir"]["binders"]
                    .as_array()
                    .expect("binders array")
                    .iter()
                    .map(|binder| binder["lean_type"].clone())
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({ "atoms": atoms })
}

fn proof_certificate(
    fixture: &std::path::Path,
    dir: &std::path::Path,
    extra: &[&str],
) -> serde_json::Value {
    let cert_path = dir.join(format!("cert{}.json", extra.len()));
    let mut args = vec!["--proof-cert", "--output", cert_path.to_str().unwrap()];
    args.extend_from_slice(extra);
    let output = run_verify(fixture, dir, &args);
    assert_verified(&output, "certificate generation");
    serde_json::from_str(&std::fs::read_to_string(&cert_path).expect("read certificate"))
        .expect("parse certificate")
}

/// The committed golden file pins the default-mode certificate of atoms that do
/// not use bit-vector semantics. Any leak of the `BV(64)` encoding into default
/// verification changes their proof hash, binder types or lowering rules, and
/// fails here.
#[test]
fn default_mode_certificates_have_no_regression() {
    let dir = temp_dir("golden");
    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let fixture = fixture_dir.join("bitvec_backward_compat.mm");

    let actual = certificate_summary(&proof_certificate(&fixture, &dir, &[]));
    let expected: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_dir.join("bitvec_backward_compat.golden.json"))
            .expect("read golden certificate"),
    )
    .expect("parse golden certificate");

    assert_eq!(
        actual,
        expected,
        "default-mode certificate drift:\nactual:\n{}",
        serde_json::to_string_pretty(&actual).unwrap()
    );

    std::fs::remove_dir_all(dir).ok();
}

/// The semantic mode is part of the verification cache key, so a result proved
/// in `Int` mode is never reused for a `--bitvec-i64` run (and vice versa).
#[test]
fn bitvec_mode_is_part_of_the_verification_cache_key() {
    let dir = temp_dir("cache");
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/bitvec_backward_compat.mm"),
    )
    .expect("read fixture");
    // Verify inside the temp dir so the cache is local to this test.
    let fixture = write_fixture(&dir, "cached.mm", &source);

    let first = run_verify(&fixture, &dir, &[]);
    assert_verified(&first, "first default-mode run");
    let cached = run_verify(&fixture, &dir, &[]);
    assert_verified(&cached, "second default-mode run");
    let cached_stdout = String::from_utf8_lossy(&cached.stdout);
    assert!(
        cached_stdout.contains("skipped (unchanged)"),
        "an unchanged module must hit the cache:\n{cached_stdout}"
    );

    let bitvec = run_verify(&fixture, &dir, &["--bitvec-i64"]);
    assert_verified(&bitvec, "bit-vector-mode run of the same module");
    let bitvec_stdout = String::from_utf8_lossy(&bitvec.stdout);
    assert!(
        !bitvec_stdout.contains("skipped (unchanged)"),
        "switching to --bitvec-i64 must invalidate the Int-mode cache:\n{bitvec_stdout}"
    );

    std::fs::remove_dir_all(dir).ok();
}
