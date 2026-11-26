use super::*;
use serde_json::{json, Value};

const FIXTURES: &str =
    include_str!("../../../tests/fixtures/verification-evidence/synthetic_cases.json");

fn record() -> Value {
    serde_json::from_str::<Value>(FIXTURES).unwrap()["cases"][0]["evidence"].clone()
}

fn requirement() -> Value {
    json!({
        "schema_version": REQUIREMENT_SCHEMA,
        "subject_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        "obligation_id": "synthetic-clamp-postcondition",
        "semantics": {
            "profile": "synthetic-pure-integer/v1",
            "integer_model": "mathematical_integer",
            "target_triple": null,
        },
        "minimum_assurance": "unbounded_proof",
        "correspondence_status": ["validated", "assumed"],
        "excluded_assumptions": [],
    })
}

fn decode_req(value: &Value) -> Result<Requirement, AcceptanceError> {
    decode_requirement(&serde_json::to_vec(value).unwrap())
}

fn run(req: &Value, rec: &Value) -> Evaluation {
    let requirement = decode_req(req).unwrap();
    let evidence = verification_evidence::decode(&serde_json::to_vec(rec).unwrap()).unwrap();
    evaluate(&requirement, &evidence)
}

fn check<'a>(evaluation: &'a Evaluation, name: &str) -> &'a str {
    evaluation
        .checks
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("missing check {name}"))
        .outcome
}

#[test]
fn requirement_requires_closed_shape_and_vocabulary() {
    let mut bad = json!("not an object");
    assert!(matches!(
        decode_req(&bad),
        Err(AcceptanceError::InvalidRequirement)
    ));
    for (key, value) in [
        ("subject_sha256", json!("short")),
        (
            "subject_sha256",
            json!("AB00000000000000000000000000000000000000000000000000000000000000"),
        ),
        ("obligation_id", json!("")),
        ("minimum_assurance", json!("proof")),
        ("correspondence_status", json!([])),
        ("correspondence_status", json!(["validated", "validated"])),
        ("correspondence_status", json!(["unsupported"])),
        ("excluded_assumptions", json!(["a", "a"])),
    ] {
        bad = requirement();
        bad[key] = value.clone();
        assert!(
            matches!(decode_req(&bad), Err(AcceptanceError::InvalidRequirement)),
            "{key} = {value}"
        );
    }
    bad = requirement();
    bad["extra"] = json!(true);
    assert!(matches!(
        decode_req(&bad),
        Err(AcceptanceError::InvalidRequirement)
    ));
    bad = requirement();
    bad.as_object_mut().unwrap().remove("minimum_assurance");
    assert!(matches!(
        decode_req(&bad),
        Err(AcceptanceError::InvalidRequirement)
    ));
}

#[test]
fn requirement_rejects_oversized_and_non_object_input() {
    assert!(matches!(
        decode_requirement(&vec![b' '; MAX_INPUT_BYTES + 1]),
        Err(AcceptanceError::InputTooLarge)
    ));
    assert!(matches!(
        decode_requirement(b"[]"),
        Err(AcceptanceError::InvalidRequirement)
    ));
    assert!(matches!(
        decode_requirement(b"{}"),
        Err(AcceptanceError::InvalidRequirement)
    ));
}

#[test]
fn clean_record_accepted_at_matching_floor() {
    for floor in [
        "unbounded_proof",
        "bounded_proof",
        "bounded_check",
        "observation",
    ] {
        let mut req = requirement();
        req["minimum_assurance"] = json!(floor);
        let evaluation = run(&req, &record());
        assert_eq!(
            evaluation.verdict,
            Verdict::Accepted,
            "floor {floor}: {:?}",
            evaluation
                .checks
                .iter()
                .map(|c| (c.name, c.outcome))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn pin_mismatches_reject_affirmatively() {
    for (key, check_name) in [
        ("subject_sha256", "subject_pin"),
        ("obligation_id", "obligation_pin"),
    ] {
        let mut req = requirement();
        req[key] = json!("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
        let evaluation = run(&req, &record());
        assert_eq!(evaluation.verdict, Verdict::Rejected);
        assert_eq!(check(&evaluation, check_name), "failed");
    }
    for (key, check_name, value) in [
        ("profile", "profile_pin", "other-profile/v1"),
        ("integer_model", "integer_model_pin", "wrapping_i64"),
        (
            "target_triple",
            "target_triple_pin",
            "x86_64-unknown-linux-gnu",
        ),
    ] {
        let mut req = requirement();
        req["semantics"][key] = json!(value);
        let evaluation = run(&req, &record());
        assert_eq!(evaluation.verdict, Verdict::Rejected, "{key}");
        assert_eq!(check(&evaluation, check_name), "failed");
    }
}

#[test]
fn unpinned_target_triple_emits_no_check() {
    let evaluation = run(&requirement(), &record());
    assert!(evaluation
        .checks
        .iter()
        .all(|entry| entry.name != "target_triple_pin"));
}

#[test]
fn negative_results_reject_at_any_floor() {
    for (method, scope, bounds, result) in [
        ("deductive_proof", "unbounded", json!([]), "counterexample"),
        (
            "bounded_model_checking",
            "bounded",
            json!(["k <= 8"]),
            "counterexample",
        ),
        ("static_heuristic", "unbounded", json!([]), "finding"),
    ] {
        let mut rec = record();
        rec["method"] = json!(method);
        rec["scope"]["kind"] = json!(scope);
        rec["scope"]["bounds"] = bounds;
        rec["result"] = json!(result);
        let evaluation = run(&requirement(), &rec);
        assert_eq!(evaluation.verdict, Verdict::Rejected, "{method}/{result}");
        assert_eq!(check(&evaluation, "result_polarity"), "failed");
    }
}

#[test]
fn non_completed_execution_is_inconclusive_not_rejected() {
    for execution in ["timeout", "resource_limit", "tool_error", "not_run"] {
        let mut rec = record();
        rec["execution"] = json!(execution);
        rec["result"] = json!("unknown");
        let evaluation = run(&requirement(), &rec);
        assert_eq!(evaluation.verdict, Verdict::Inconclusive, "{execution}");
        assert_eq!(check(&evaluation, "execution_completed"), "indeterminate");
        assert_eq!(check(&evaluation, "result_polarity"), "indeterminate");
    }
}

#[test]
fn unknown_unsupported_not_checked_results_are_inconclusive() {
    for result in ["unknown", "unsupported", "not_checked"] {
        let mut rec = record();
        rec["result"] = json!(result);
        rec["correspondence"]["relation"] = json!("unchecked");
        rec["correspondence"]["status"] = json!("unverified");
        let evaluation = run(&requirement(), &rec);
        assert_eq!(evaluation.verdict, Verdict::Inconclusive, "{result}");
    }
}

#[test]
fn finite_execution_satisfies_observation_but_never_proof() {
    let mut rec = record();
    rec["method"] = json!("finite_execution");
    rec["scope"] = json!({"kind": "finite", "bounds": ["input_count <= 4"]});
    rec["result"] = json!("observed_no_failure");

    let mut req = requirement();
    req["minimum_assurance"] = json!("observation");
    assert_eq!(run(&req, &rec).verdict, Verdict::Accepted);

    for floor in ["unbounded_proof", "bounded_proof", "bounded_check"] {
        req["minimum_assurance"] = json!(floor);
        let evaluation = run(&req, &rec);
        assert_eq!(evaluation.verdict, Verdict::Rejected, "floor {floor}");
        assert_eq!(check(&evaluation, "method_sufficient"), "failed");
    }
}

#[test]
fn bounded_proof_record_does_not_satisfy_unbounded_floor() {
    let mut rec = record();
    rec["method"] = json!("bounded_model_checking");
    rec["scope"] = json!({"kind": "bounded", "bounds": ["k <= 8"]});
    let evaluation = run(&requirement(), &rec);
    assert_eq!(check(&evaluation, "method_sufficient"), "failed");
    assert_eq!(evaluation.verdict, Verdict::Rejected);

    let mut req = requirement();
    req["minimum_assurance"] = json!("bounded_check");
    assert_eq!(run(&req, &rec).verdict, Verdict::Accepted);

    req["minimum_assurance"] = json!("unbounded_proof");
    let evaluation = run(&req, &rec);
    assert_eq!(check(&evaluation, "scope_sufficient"), "failed");
    assert_eq!(evaluation.verdict, Verdict::Rejected);
}

#[test]
fn identity_floor_accepts_only_identity_checks() {
    let mut rec = record();
    rec["method"] = json!("identity_check");
    rec["scope"] = json!({"kind": "identity", "bounds": []});
    let mut req = requirement();
    req["minimum_assurance"] = json!("identity");
    assert_eq!(run(&req, &rec).verdict, Verdict::Accepted);
    req["minimum_assurance"] = json!("unbounded_proof");
    assert_eq!(run(&req, &rec).verdict, Verdict::Rejected);
    // A proof does not substitute for an identity binding either way.
    req["minimum_assurance"] = json!("identity");
    assert_eq!(run(&req, &record()).verdict, Verdict::Rejected);
}

#[test]
fn correspondence_gate_lists_allowed_statuses() {
    // Fixture record: relation exact_model, status assumed.
    let evaluation = run(&requirement(), &record());
    assert_eq!(check(&evaluation, "correspondence_status"), "passed");

    let mut req = requirement();
    req["correspondence_status"] = json!(["validated"]);
    let evaluation = run(&req, &record());
    assert_eq!(check(&evaluation, "correspondence_status"), "indeterminate");
    assert_eq!(evaluation.verdict, Verdict::Inconclusive);

    let mut rec = record();
    rec["correspondence"]["relation"] = json!("unsupported");
    rec["correspondence"]["status"] = json!("unsupported");
    rec["result"] = json!("unsupported");
    let evaluation = run(&requirement(), &rec);
    assert_eq!(check(&evaluation, "correspondence_status"), "failed");
    assert_eq!(evaluation.verdict, Verdict::Rejected);
}

#[test]
fn excluded_assumptions_reject_on_intersection() {
    let mut req = requirement();
    req["excluded_assumptions"] = json!(["Input is a signed 64-bit integer."]);
    let evaluation = run(&req, &record());
    assert_eq!(check(&evaluation, "excluded_assumptions"), "failed");
    assert_eq!(evaluation.verdict, Verdict::Rejected);
}

#[test]
fn rejected_and_inconclusive_checks_do_not_hide_each_other() {
    let mut req = requirement();
    req["obligation_id"] = json!("other-obligation");
    let mut rec = record();
    rec["result"] = json!("unknown");
    rec["correspondence"]["relation"] = json!("unchecked");
    rec["correspondence"]["status"] = json!("unverified");
    let evaluation = run(&req, &rec);
    assert_eq!(evaluation.verdict, Verdict::Rejected);
    assert_eq!(check(&evaluation, "obligation_pin"), "failed");
    assert_eq!(check(&evaluation, "result_polarity"), "indeterminate");
}

#[test]
fn promotable_gate_accepts_oversized_valid_descriptor() {
    let requirement_list = serde_json::to_vec(&json!({
        "schema_version": "mumei.evidence-requirement-list/v1",
        "requirements": [requirement()],
    }))
    .unwrap();
    let evidence = serde_json::to_vec(&json!({
        "schema_version": "mumei.verification-evidence-list/v1",
        "evidence": [],
    }))
    .unwrap();
    let descriptor = serde_json::to_vec(&json!({
        "schema_version": "mumei.evidence-requirement-list-acceptance/v1",
        "verdict": "accepted",
        "requirement_schema_version": "mumei.evidence-requirement-list/v1",
        "requirement_list_fingerprint": format!("{:x}", Sha256::digest(&requirement_list)),
        "evidence_schema_version": "mumei.verification-evidence-list/v1",
        "list_fingerprint": format!("{:x}", Sha256::digest(&evidence)),
        "consistent": true,
        "conflict_count": 0,
        // Per-requirement per-record check outcomes make a real
        // descriptor larger than the single-document input bound; the
        // gate must not reject it for size alone.
        "per_requirement": "x".repeat(MAX_INPUT_BYTES * 2),
    }))
    .unwrap();
    assert!(descriptor.len() > MAX_INPUT_BYTES);
    assert!(descriptor.len() <= MAX_ACCEPTANCE_DESCRIPTOR_BYTES);
    assert!(promotable_failures(&requirement_list, &evidence, &descriptor).is_empty());
}
