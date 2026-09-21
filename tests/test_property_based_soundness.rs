//! Property-based validation as a soundness cross-check for array atoms.
//!
//! Each atom below stores into its array parameter through one of the havoc
//! paths that produced the 2026-09-20 stale-read false negatives (direct
//! store, `while`-loop store, `while`-loop sweep). Callee- and lambda-based
//! stores need sibling atoms / native execution and are covered by
//! `test_differential_fuzz.rs` instead. The verifier
//! must prove the atom, and property-based execution over random arrays must
//! then find *no* counterexample: a stale-read false negative would surface
//! here as `verified` on the Z3 side and `failed` on the concrete side.
//!
//! The mirror check pins the direction: the off-by-one twin of each ensures
//! must be rejected by the verifier *and* refuted concretely, so the
//! cross-check is known to be able to disagree with a wrong claim.

use std::fs;
use std::process::Command;

use mumei_core::parser::{parse_atom, parse_module, Item};
use mumei_core::verification::{run_property_based_test, ModuleEnv, PropertyBasedTestConfig};

struct Case {
    name: &'static str,
    good_ensures: &'static str,
    bad_ensures: &'static str,
    body: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "direct_store",
        good_ensures: "result == 9",
        bad_ensures: "result == 10",
        body: "a[0] = 9;\n    a[0]",
    },
    Case {
        name: "loop_store",
        good_ensures: "result == 4",
        bad_ensures: "result == 5",
        body: "let i = 0;\n    while i < 2\n    invariant: i >= 0 && i <= 2 && (i == 0 || a[1] == 4)\n    decreases: 2 - i\n    {\n        a[1] = 4;\n        i = i + 1\n    };\n    a[1]",
    },
    Case {
        name: "loop_sweep",
        good_ensures: "result == 6",
        bad_ensures: "result == 7",
        body: "let i = 0;\n    while i < 3\n    invariant: i >= 0 && i <= 3 && forall(j, 0, i, a[j] == 6)\n    decreases: 3 - i\n    {\n        a[i] = 6;\n        i = i + 1\n    };\n    a[2]",
    },
];

fn atom_source(case: &Case, ensures: &str) -> String {
    format!(
        "atom {}(a: [i64]) -> i64\nrequires: len(a) >= 3;\nensures: {ensures};\nbody: {{\n    {}\n}};\n",
        case.name, case.body
    )
}

fn verify(source: &str, tag: &str) -> bool {
    let dir = std::env::temp_dir().join(format!("mumei_pbt_sound_{}_{tag}", std::process::id()));
    fs::create_dir_all(&dir).expect("fixture dir");
    let file = dir.join("main.mm");
    fs::write(&file, source).expect("fixture");
    let out = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg(&file)
        .current_dir(&dir)
        .output()
        .expect("run mumei verify");
    let _ = fs::remove_dir_all(&dir);
    out.status.success()
}

fn property_check(source: &str, seed: u64) -> String {
    let items = parse_module(source);
    assert!(
        items.iter().any(|item| matches!(item, Item::Atom(_))),
        "fixture did not parse as an atom:\n{source}"
    );
    let atom = parse_atom(source);
    let config = PropertyBasedTestConfig {
        test_count: 48,
        max_shrink_steps: 32,
        seed,
        include_boundary_values: true,
    };
    let result = run_property_based_test(&atom, &ModuleEnv::new(), &config);
    assert!(
        result.tests_run > 0,
        "no property tests executed for:\n{source}"
    );
    result.status
}

#[test]
fn verified_array_atoms_are_not_refuted_by_property_based_validation() {
    for (n, case) in CASES.iter().enumerate() {
        let source = atom_source(case, case.good_ensures);
        assert!(
            verify(&source, &format!("good_{}", case.name)),
            "{}: verifier rejected the reference-correct atom:\n{source}",
            case.name
        );
        let status = property_check(&source, 0x5EED_0000 + n as u64);
        assert_eq!(
            status, "passed",
            "{}: property-based execution refuted a reference-correct claim (verifier said verified):\n{source}",
            case.name
        );
    }
}

#[test]
fn stale_array_claims_are_rejected_by_both_checkers() {
    for (n, case) in CASES.iter().enumerate() {
        let source = atom_source(case, case.bad_ensures);
        assert!(
            !verify(&source, &format!("bad_{}", case.name)),
            "{}: verifier accepted a stale-read claim:\n{source}",
            case.name
        );
        let status = property_check(&source, 0xBAD_0000 + n as u64);
        assert_eq!(
            status, "failed",
            "{}: property-based execution could not refute the stale claim:\n{source}",
            case.name
        );
    }
}
