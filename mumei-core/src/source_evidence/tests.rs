use super::*;

fn saved(source: &str, options: Options) -> Vec<u8> {
    serde_json::to_vec(&established(source, options)).unwrap()
}

#[test]
fn saved_source_recheck_matches_decoded_fields_not_json_formatting() {
    let bytes = saved(CLAMP, Options::default());
    assert_eq!(
        recheck(CLAMP, &bytes, Options::default()).unwrap(),
        RecheckOutcome::Consistent
    );
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        recheck(
            CLAMP,
            &serde_json::to_vec_pretty(&value).unwrap(),
            Options::default()
        )
        .unwrap(),
        RecheckOutcome::Consistent
    );
    let custom = Options {
        timeout_ms: 9000,
        resource_limit: 900_000,
    };
    assert_eq!(
        recheck(CLAMP, &saved(CLAMP, custom), custom).unwrap(),
        RecheckOutcome::Consistent
    );
    assert_eq!(
        recheck(CLAMP, &bytes, custom),
        Err(SourceEvidenceError::InconsistentSavedEvidence)
    );
    assert_eq!(
        recheck(
            &format!("{CLAMP}\n// relocated snapshot bytes\n"),
            &bytes,
            Options::default()
        ),
        Err(SourceEvidenceError::InconsistentSavedEvidence)
    );
}

#[test]
fn saved_source_recheck_checks_every_leaf_including_assumptions_and_exclusions() {
    fn leaves(value: &serde_json::Value, path: String, paths: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(object) => {
                for (key, value) in object {
                    leaves(value, format!("{path}/{key}"), paths);
                }
            }
            serde_json::Value::Array(array) if !array.is_empty() => {
                for (index, value) in array.iter().enumerate() {
                    leaves(value, format!("{path}/{index}"), paths);
                }
            }
            _ => paths.push(path),
        }
    }
    let original: serde_json::Value =
        serde_json::from_slice(&saved(CLAMP, Options::default())).unwrap();
    let mut paths = Vec::new();
    leaves(&original, String::new(), &mut paths);
    assert!(paths.len() >= 30);
    for path in paths {
        let mut mutated = original.clone();
        let field = mutated.pointer_mut(&path).unwrap();
        *field = match field {
            serde_json::Value::String(s) if s.len() == 64 => {
                serde_json::Value::String("f".repeat(64))
            }
            serde_json::Value::Array(_) => serde_json::json!(["FAKE_PRIVATE_ARTIFACT"]),
            _ => serde_json::Value::String("FAKE_PRIVATE_METADATA".into()),
        };
        assert!(
            recheck(
                CLAMP,
                &serde_json::to_vec(&mutated).unwrap(),
                Options::default()
            )
            .is_err(),
            "unchecked field: {path}"
        );
    }
}

#[test]
fn rebound_success_cannot_replace_fresh_refutation_or_inconclusive_queries() {
    let source = CLAMP.replace("body: { if x < 0 { 0 } else { x } };", "body: { 1 };");
    assert_ne!(source, CLAMP);
    let contract =
        semantic_contract::extract_pure_i64_contract(&source, SOURCE_LABEL, None, false).unwrap();
    let mut forged = established(CLAMP, Options::default());
    forged.subject.sha256 = contract.source_sha256;
    let bytes = serde_json::to_vec(&forged).unwrap();
    assert_eq!(
        recheck(&source, &bytes, Options::default()).unwrap(),
        RecheckOutcome::Refuted
    );
    assert_eq!(
        recheck(
            CLAMP,
            &bytes,
            Options {
                resource_limit: 1,
                ..Options::default()
            }
        )
        .unwrap(),
        RecheckOutcome::Inconclusive
    );
    assert_eq!(
        recheck(
            CLAMP,
            &bytes,
            Options {
                timeout_ms: 0,
                ..Options::default()
            }
        ),
        Err(SourceEvidenceError::InvalidBudget)
    );
    for source in [
        "",
        "atom f() -> i64 body: { 0 };",
        "atom f() -> i64 requires: false; ensures: false; body: { 0 };",
    ] {
        assert_eq!(
            recheck(source, &bytes, Options::default()),
            Err(SourceEvidenceError::UnsupportedSource)
        );
    }
}

#[test]
fn saved_source_recheck_rejects_malformed_duplicate_unknown_and_oversized_records() {
    let bytes = saved(CLAMP, Options::default());
    let text = String::from_utf8(bytes.clone()).unwrap();
    for input in [
        Vec::new(),
        vec![0xff],
        b"{}".to_vec(),
        b"[]".to_vec(),
        format!("{text} {text}").into_bytes(),
        text.replacen("{", "{\"schema_version\":\"FAKE_PRIVATE_DUPLICATE\",", 1)
            .into_bytes(),
        text.replacen(
            "{",
            "{\"schema_versi\\u006fn\":\"FAKE_PRIVATE_DUPLICATE\",",
            1,
        )
        .into_bytes(),
        text.replacen("{", "{\"FAKE_PRIVATE_UNKNOWN\":true,", 1)
            .into_bytes(),
        vec![b' '; crate::verification_evidence::MAX_INPUT_BYTES + 1],
    ] {
        assert_eq!(
            recheck(CLAMP, &input, Options::default()),
            Err(SourceEvidenceError::InvalidSavedEvidence)
        );
    }
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["semantics"]
        .as_object_mut()
        .unwrap()
        .remove("target_triple");
    assert_eq!(
        recheck(
            CLAMP,
            &serde_json::to_vec(&value).unwrap(),
            Options::default()
        ),
        Err(SourceEvidenceError::InvalidSavedEvidence)
    );
}

const CLAMP: &str = include_str!("../../../tests/fixtures/semantic_contract_clamp.mm");

fn established(source: &str, options: Options) -> Evidence {
    match verify(source, options).unwrap() {
        Outcome::EstablishedWithinProfile(record) => *record,
        other => panic!("expected fresh scoped evidence, got {other:?}"),
    }
}

#[test]
fn fresh_record_has_explicit_scope_trust_and_actual_linked_tool_version() {
    let evidence = established(CLAMP, Options::default());
    let contract =
        semantic_contract::extract_pure_i64_contract(CLAMP, SOURCE_LABEL, None, false).unwrap();
    assert_eq!(evidence.subject.sha256, contract.source_sha256);
    assert_eq!(evidence.specification.sha256, contract.specification_sha256);
    assert_eq!(evidence.subject.id, SOURCE_LABEL);
    assert_eq!(evidence.subject.kind, "mumei_source");
    assert_eq!(evidence.semantics.profile, PURE_I64_PROFILE);
    assert_eq!(evidence.semantics.integer_model, "wrapping_i64");
    assert_eq!(evidence.semantics.target_triple, None);
    assert_eq!(evidence.method, "deductive_proof");
    assert_eq!(evidence.execution, "completed");
    assert_eq!(evidence.result, "established");
    assert_eq!(evidence.scope.kind, "unbounded");
    assert!(evidence.scope.bounds.is_empty());
    assert_eq!(evidence.correspondence.relation, "exact_model");
    assert_eq!(evidence.correspondence.status, "assumed");
    assert!(evidence.correspondence.notes[0].contains("trusted_not_independently_proved"));
    assert!(!evidence.assumptions.is_empty());
    assert!(!evidence.exclusions.is_empty());
    assert!(evidence.artifact_paths.is_empty());
    assert_eq!(
        evidence.provenance.adapter.protocol_version,
        PRODUCER_PROFILE
    );
    let v = crate::verification::linked_z3_version();
    assert_eq!(
        evidence.provenance.tool.version,
        format!("{}.{}.{}.{}", v.0, v.1, v.2, v.3)
    );
    assert_eq!(evidence.provenance.tool.name, "Z3");
    assert_eq!(evidence.provenance.translator_version, None);
    assert_eq!(evidence.provenance.bridge_lemma_hash, None);
    let encoded = serde_json::to_vec(&evidence).unwrap();
    assert!(encoded.len() < crate::verification_evidence::MAX_INPUT_BYTES);
    assert_eq!(
        crate::verification_evidence::decode(&encoded).unwrap(),
        evidence
    );
}

#[test]
fn identities_bind_bytes_specification_and_budgets_without_raw_names() {
    let base = established(CLAMP, Options::default());
    assert_eq!(base, established(CLAMP, Options::default()));
    let renamed = established(
        &CLAMP.replace("clamp_nonnegative", "FAKE_PRIVATE_ATOM"),
        Options::default(),
    );
    assert_ne!(base.subject.sha256, renamed.subject.sha256);
    assert!(base.specification == renamed.specification);
    assert!(!serde_json::to_string(&renamed)
        .unwrap()
        .contains("FAKE_PRIVATE"));
    let source_changed = established(
        &format!("{CLAMP}\n// whitespace and comment identity\n"),
        Options::default(),
    );
    assert_ne!(base.subject.sha256, source_changed.subject.sha256);
    assert!(base.specification == source_changed.specification);
    let spec_changed = established(
        &CLAMP.replace("ensures:", "ensures: true &&"),
        Options::default(),
    );
    assert_ne!(base.specification.sha256, spec_changed.specification.sha256);
    assert_ne!(base.obligation_id, spec_changed.obligation_id);
    for options in [
        Options {
            timeout_ms: 9_000,
            ..Options::default()
        },
        Options {
            resource_limit: 900_000,
            ..Options::default()
        },
    ] {
        let record = established(CLAMP, options);
        assert!(base.subject == record.subject);
        assert!(base.specification == record.specification);
        assert_ne!(
            base.provenance.configuration_fingerprint,
            record.provenance.configuration_fingerprint
        );
    }
}

#[test]
fn every_supported_comparison_and_boolean_connective_works_over_two_inputs() {
    for comparison in ["<", "<=", ">", ">=", "==", "!="] {
        let source = format!("atom select(x: i64, y: i64) -> i64 requires: true; ensures: (x {comparison} y => result == x) && (!(x {comparison} y) => result == y); body: {{ if x {comparison} y {{ x }} else {{ y }} }};");
        established(&source, Options::default());
        let wrong = source.replace("result == x", "result != x");
        assert_eq!(
            verify(&wrong, Options::default()).unwrap(),
            Outcome::Refuted
        );
    }
    for expression in ["x < 0 && x != 1", "x < 0 || x == 1", "!(x >= 0)"] {
        let source = format!("atom boolean(x: i64) -> i64 requires: true; ensures: ({expression} => result == 0) && (!({expression}) => result == x); body: {{ if {expression} {{ 0 }} else {{ x }} }};");
        established(&source, Options::default());
    }
}

#[test]
fn full_i64_domain_not_finite_observations_or_unbounded_integers() {
    for value in ["0", "1", "9223372036854775807"] {
        established(&format!("atom constant() -> i64 requires: true; ensures: result == {value}; body: {{ {value} }};"), Options::default());
        let source = format!("atom identity(x: i64) -> i64 requires: true; ensures: x == {value} => result == 0; body: {{ 1 }};");
        assert_eq!(
            verify(&source, Options::default()).unwrap(),
            Outcome::Refuted
        );
    }
    let source = "atom missed_by_replay(x: i64) -> i64 requires: true; ensures: x == 42 => result == 0; body: { 1 };";
    assert_eq!(
        verify(source, Options::default()).unwrap(),
        Outcome::Refuted
    );
    established("atom identity(x: i64) -> i64 requires: true; ensures: result == x && result <= 9223372036854775807; body: { x };", Options::default());
    let negative =
        "atom identity(x: i64) -> i64 requires: true; ensures: result >= 0; body: { x };";
    assert_eq!(
        verify(negative, Options::default()).unwrap(),
        Outcome::Refuted
    );
    let context = Context::new(&Config::new());
    let body = semantic_contract::encode_pure_i64_body(negative, &context, false).unwrap();
    let obligation = semantic_contract::encode_pure_i64_obligation(negative, &context).unwrap();
    for value in [i64::MIN, -1] {
        use z3::ast::Ast;
        let solver = Solver::new(&context);
        solver.assert(&obligation);
        solver.assert(&body.inputs[0]._eq(&z3::ast::BV::from_i64(&context, value, 64)));
        assert_eq!(solver.check(), SatResult::Sat);
    }
}

#[test]
fn omitted_or_repeated_contract_clauses_do_not_become_implicit_true_evidence() {
    for source in [
        "atom f(x: i64) -> i64 body: { x };",
        "atom f(x: i64) -> i64 requires: true; body: { x };",
        "atom f(x: i64) -> i64 ensures: result == x; body: { x };",
        "atom f(x: i64) -> i64 requires: true; ensures: true;",
        "atom f(x: i64) -> i64 requires: ; ensures: true; body: { x };",
        "atom f(x: i64) -> i64 requires: true; ensures: // empty\n; body: { x };",
        "atom f(x: i64) -> i64 { body: { x } }",
        "// requires: true; ensures: true;\natom f(x: i64) -> i64 body: { x };",
        "atom f(x: i64) -> i64 requires: true; requires: true; ensures: true; body: { x };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; ensures: true; body: { x };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { x }; body: { 0 };",
    ] {
        assert_eq!(
            verify(source, Options::default()),
            Err(SourceEvidenceError::UnsupportedSource)
        );
    }
    for (index, source) in [
        "atom f(x: i64) -> i64 { // requires: false; ensures: false;\n requires: true; ensures: result == x; body: { x } }",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { x };",
    ].into_iter().enumerate() {
        assert!(explicit_contract_clauses(source).unwrap(), "token-presence case {index}");
        let context = Context::new(&Config::new());
        let encoded = semantic_contract::encode_pure_i64_obligation(source, &context);
        assert!(encoded.is_ok(), "C03 encoding case {index}: {:?}", encoded.err());
        established(source, Options::default());
    }
}

#[test]
fn false_and_nonliteral_preconditions_cannot_succeed_vacuously() {
    for precondition in [
        "false",
        "x != x",
        "x < x",
        "true && false",
        "x >= 0",
        "true && true",
        "(true)",
    ] {
        let source = format!("atom conditional(x: i64) -> i64 requires: {precondition}; ensures: false; body: {{ x }};");
        assert_eq!(
            verify(&source, Options::default()),
            Err(SourceEvidenceError::UnsupportedSource)
        );
    }
    assert_eq!(
        verify(
            &CLAMP.replace("ensures:", "ensures: false &&"),
            Options::default()
        )
        .unwrap(),
        Outcome::Refuted
    );
}

#[test]
fn malformed_effectful_lossy_or_unsupported_sources_fail_closed() {
    for source in [
        "", "FAKE_PRIVATE_MALFORMED", "atom f(x: i64) -> i64 requires: true; ensures: result == x; body: {",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { x + 1 };",
        "atom f(x: i64) -> i64 requires: true; ensures: result == x + 0; body: { x };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { x / 0 };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { f(x) };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { let y = x; y };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { while true { x }; x };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { x[0] };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { 1.5 };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { 9223372036854775808 };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { -9223372036854775809 };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; effects: [Log]; body: { x };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { missing };",
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: { true };",
        "atom f(x: i64) -> i64 requires: true; ensures: 1; body: { x };",
        "atom f(x: u64) -> i64 requires: true; ensures: true; body: { x };",
        "atom f(x: bool) -> i64 requires: true; ensures: true; body: { 0 };",
        "atom f(x: i64) -> u64 requires: true; ensures: true; body: { 0 };",
        "atom f(x: i64, x: i64) -> i64 requires: true; ensures: true; body: { 0 };",
        "atom f(result: i64) -> i64 requires: true; ensures: true; body: { 0 };",
        "import \"FAKE_PRIVATE_IMPORT\"; atom f() -> i64 requires: true; ensures: true; body: { 0 };",
    ] {
        assert_eq!(verify(source, Options::default()), Err(SourceEvidenceError::UnsupportedSource));
    }
    assert_eq!(
        verify(&format!("{CLAMP}\n{CLAMP}"), Options::default()),
        Err(SourceEvidenceError::UnsupportedSource)
    );
}

#[test]
fn source_byte_token_depth_and_parameter_limits_are_enforced() {
    let mut exact = CLAMP.to_owned();
    exact.push_str(&" ".repeat(crate::verification_evidence::MAX_INPUT_BYTES - exact.len()));
    established(&exact, Options::default());
    assert_eq!(
        verify(&(exact + " "), Options::default()),
        Err(SourceEvidenceError::UnsupportedSource)
    );
    let many_tokens = format!(
        "atom f(x: i64) -> i64 requires: true; ensures: {}; body: {{ x }};",
        vec!["true"; 130].join(" && ")
    );
    let deep = format!(
        "atom f(x: i64) -> i64 requires: true; ensures: true; body: {{ {}x{} }};",
        "(".repeat(49),
        ")".repeat(49)
    );
    for source in [many_tokens, deep] {
        assert_eq!(
            verify(&source, Options::default()),
            Err(SourceEvidenceError::UnsupportedSource)
        );
    }
    for count in [16, 17] {
        let binders = (0..count)
            .map(|i| format!("x{i}: i64"))
            .collect::<Vec<_>>()
            .join(", ");
        let source = format!("atom identity({binders}) -> i64 requires: true; ensures: result == x0; body: {{ x0 }};");
        if count == 16 {
            established(&source, Options::default());
        } else {
            assert_eq!(
                verify(&source, Options::default()),
                Err(SourceEvidenceError::UnsupportedSource)
            );
        }
    }
}

#[test]
fn invalid_or_unlimited_budgets_are_rejected_before_querying() {
    for (timeout_ms, resource_limit) in [
        (0, 1),
        (1, 0),
        (10_001, 1),
        (1, 1_000_001),
        (u32::MAX, 1),
        (1, u32::MAX),
    ] {
        assert_eq!(
            verify(
                CLAMP,
                Options {
                    timeout_ms,
                    resource_limit
                }
            ),
            Err(SourceEvidenceError::InvalidBudget)
        );
    }
}

#[test]
fn resource_exhaustion_and_all_non_unsat_outcomes_emit_no_record() {
    assert_eq!(
        verify(
            CLAMP,
            Options {
                resource_limit: 1,
                ..Options::default()
            }
        )
        .unwrap(),
        Outcome::Inconclusive
    );
    let contract =
        semantic_contract::extract_pure_i64_contract(CLAMP, SOURCE_LABEL, None, false).unwrap();
    assert_eq!(
        finish(SatResult::Unknown, &contract, Options::default()).unwrap(),
        Outcome::Inconclusive
    );
    assert_eq!(
        finish(SatResult::Sat, &contract, Options::default()).unwrap(),
        Outcome::Refuted
    );
}
