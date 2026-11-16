use crate::cli::EvidenceCommand;
use mumei_core::source_evidence::{self, AcceptOutcome, Options, Outcome, RecheckOutcome};
use mumei_core::verification_evidence::{self, MAX_INPUT_BYTES, SCHEMA_VERSION};
use std::{io::Write, path::Path};

const TEXT: &str = "Evidence v1 representation valid.\n\
Scope: representation only; no claim truth, file identity, freshness, producer authenticity, semantic correspondence or artifact semantics checked.\n\
Evidence acceptance: not evaluated.\n\
No verifier or artifact executed; no grade promotion or final-executable assurance.\n";

pub(crate) fn run(command: EvidenceCommand) {
    let mut output = std::io::stdout().lock();
    let (result, prefix) = match command {
        EvidenceCommand::RecheckSource {
            input,
            source,
            json,
            solver_timeout,
            solver_resource_limit,
        } => (
            source_options(&solver_timeout, &solver_resource_limit).and_then(|options| {
                recheck_source(
                    Path::new(&input),
                    Path::new(&source),
                    json,
                    options,
                    &mut output,
                )
            }),
            "Source evidence recheck failed",
        ),
        EvidenceCommand::AcceptSource {
            input,
            source,
            json,
            solver_timeout,
            solver_resource_limit,
        } => (
            source_options(&solver_timeout, &solver_resource_limit).and_then(|options| {
                accept_source(
                    Path::new(&input),
                    Path::new(&source),
                    json,
                    options,
                    &mut output,
                )
            }),
            "Source evidence acceptance failed",
        ),
        EvidenceCommand::Check {
            input,
            stdin,
            json,
            expected_adapter,
        } => (
            check(
                input.as_deref().map(Path::new),
                stdin,
                json,
                expected_adapter.as_deref(),
                &mut output,
            )
            .map(|()| 0),
            "Evidence representation check failed",
        ),
        EvidenceCommand::CheckList {
            input,
            stdin,
            json,
            expected_adapter,
        } => (
            check_list(
                input.as_deref().map(Path::new),
                stdin,
                json,
                expected_adapter.as_deref(),
                &mut output,
            )
            .map(|()| 0),
            "Evidence list representation check failed",
        ),
        EvidenceCommand::CheckConsistency {
            input,
            stdin,
            json,
            expected_adapter,
        } => (
            check_consistency(
                input.as_deref().map(Path::new),
                stdin,
                json,
                expected_adapter.as_deref(),
                &mut output,
            )
            .map(|()| 0),
            "Evidence consistency check failed",
        ),
        EvidenceCommand::VerifySource {
            input,
            json,
            solver_timeout,
            solver_resource_limit,
        } => (
            source_options(&solver_timeout, &solver_resource_limit)
                .and_then(|options| verify_source(Path::new(&input), json, options, &mut output)),
            "Source evidence check failed",
        ),
    };
    match result {
        Ok(0) => (),
        Ok(code) => {
            eprintln!(
                "{}",
                if code == 3 {
                    "Source query inconclusive within bounded profile; no Evidence emitted"
                } else {
                    "Source postcondition refuted within restricted profile; no Evidence emitted"
                }
            );
            std::process::exit(code);
        }
        Err(error) => {
            eprintln!("{prefix}: {error}");
            std::process::exit(1);
        }
    }
}

fn recheck_source(
    input: &Path,
    source: &Path,
    json: bool,
    options: Options,
    output: &mut impl Write,
) -> Result<i32, &'static str> {
    let saved = super::input::read_regular(input, MAX_INPUT_BYTES as u64)?;
    let source = super::input::read_regular(source, MAX_INPUT_BYTES as u64)?;
    let source = String::from_utf8(source).map_err(|_| "source is not UTF-8")?;
    let outcome =
        source_evidence::recheck(&source, &saved, options).map_err(|error| match error {
            source_evidence::SourceEvidenceError::InvalidBudget => {
                "invalid bounded source-query budget"
            }
            source_evidence::SourceEvidenceError::InvalidSavedEvidence => {
                "invalid saved source evidence representation"
            }
            source_evidence::SourceEvidenceError::InconsistentSavedEvidence => {
                "saved source evidence differs from fresh same-profile result"
            }
            _ => "source outside restricted profile or cannot encode evidence",
        })?;
    write_recheck(outcome, json, output)
}

fn write_recheck(
    outcome: RecheckOutcome,
    json: bool,
    output: &mut impl Write,
) -> Result<i32, &'static str> {
    match outcome {
        RecheckOutcome::Refuted => return Ok(1),
        RecheckOutcome::Inconclusive => return Ok(3),
        RecheckOutcome::Consistent => (),
    }
    let bytes = if json {
        let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "mumei.source-evidence-recheck/v1",
            "evidence_schema_version": SCHEMA_VERSION,
            "recheck": "consistent",
            "scope": "restricted_c03_source_postcondition_and_same_profile_record_consistency",
            "acceptance": "not_evaluated",
            "checks_not_performed": ["producer_authenticity", "historical_execution", "parser_encoder_solver_correctness", "llvm_correspondence", "artifact_semantics", "grade_promotion", "lean_kernel_checks"],
        })).map_err(|_| "cannot encode source recheck diagnostic")?;
        bytes.push(b'\n');
        bytes
    } else {
        b"Source Evidence consistent with fresh restricted C03 source query and current profile/provenance.\nEvidence acceptance: not evaluated; no producer authenticity, historical execution, LLVM, artifact, grade or final-executable assurance.\n".to_vec()
    };
    output
        .write_all(&bytes)
        .map_err(|_| "cannot write source recheck diagnostic")?;
    Ok(0)
}

fn accept_source(
    input: &Path,
    source: &Path,
    json: bool,
    options: Options,
    output: &mut impl Write,
) -> Result<i32, &'static str> {
    let saved = super::input::read_regular(input, MAX_INPUT_BYTES as u64)?;
    let source = super::input::read_regular(source, MAX_INPUT_BYTES as u64)?;
    let source = String::from_utf8(source).map_err(|_| "source is not UTF-8")?;
    let outcome =
        source_evidence::accept(&source, &saved, options).map_err(|error| match error {
            source_evidence::SourceEvidenceError::InvalidBudget => {
                "invalid bounded source-query budget"
            }
            source_evidence::SourceEvidenceError::InvalidSavedEvidence => {
                "invalid saved source evidence representation"
            }
            source_evidence::SourceEvidenceError::InconsistentSavedEvidence => {
                "saved source evidence differs from fresh same-profile result"
            }
            _ => "source outside restricted profile or cannot encode evidence",
        })?;
    write_accept(outcome, json, &saved, output)
}

fn write_accept(
    outcome: AcceptOutcome,
    json: bool,
    saved: &[u8],
    output: &mut impl Write,
) -> Result<i32, &'static str> {
    match outcome {
        AcceptOutcome::Rejected => return Ok(1),
        AcceptOutcome::Inconclusive => return Ok(3),
        AcceptOutcome::Accepted => (),
    }
    let bytes = if json {
        use sha2::{Digest, Sha256};
        let evidence_fingerprint = format!("{:x}", Sha256::digest(saved));
        let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "mumei.source-evidence-acceptance/v1",
            "evidence_schema_version": SCHEMA_VERSION,
            "evidence_fingerprint": evidence_fingerprint,
            "acceptance": "evaluated",
            "accepted": true,
            "scope": "restricted_c03_source_postcondition_and_same_profile_record_consistency",
            "checks_performed": ["representation_gate", "fresh_same_profile_source_query"],
            "checks_not_performed": ["producer_authenticity", "historical_execution", "record_freshness", "parser_encoder_solver_correctness", "independent_model_correspondence", "llvm_correspondence", "artifact_semantics", "grade_promotion", "lean_kernel_checks"],
        })).map_err(|_| "cannot encode source acceptance descriptor")?;
        bytes.push(b'\n');
        bytes
    } else {
        b"Source Evidence accepted within restricted C03 source-postcondition profile: representation valid and consistent with a fresh same-profile query.\nAcceptance scope excludes producer authenticity, historical execution, record freshness, independent correspondence, LLVM, artifact, grade and final-executable assurance.\n".to_vec()
    };
    output
        .write_all(&bytes)
        .map_err(|_| "cannot write source acceptance descriptor")?;
    Ok(0)
}

fn source_options(timeout: &str, resource_limit: &str) -> Result<Options, &'static str> {
    Ok(Options {
        timeout_ms: timeout
            .parse()
            .map_err(|_| "invalid bounded source-query budget")?,
        resource_limit: resource_limit
            .parse()
            .map_err(|_| "invalid bounded source-query budget")?,
    })
}

fn verify_source(
    path: &Path,
    json: bool,
    options: Options,
    output: &mut impl Write,
) -> Result<i32, &'static str> {
    let bytes = super::input::read_regular(path, MAX_INPUT_BYTES as u64)?;
    let source = String::from_utf8(bytes).map_err(|_| "source is not UTF-8")?;
    let outcome = source_evidence::verify(&source, options).map_err(|error| match error {
        source_evidence::SourceEvidenceError::InvalidBudget => {
            "invalid bounded source-query budget"
        }
        _ => "source outside restricted profile or cannot encode evidence",
    })?;
    write_source(outcome, json, output)
}

fn write_source(
    outcome: Outcome,
    json: bool,
    output: &mut impl Write,
) -> Result<i32, &'static str> {
    let record = match outcome {
        Outcome::Refuted => return Ok(1),
        Outcome::Inconclusive => return Ok(3),
        Outcome::EstablishedWithinProfile(record) => record,
    };
    let bytes = if json {
        let mut bytes =
            serde_json::to_vec_pretty(&record).map_err(|_| "cannot encode source evidence")?;
        bytes.push(b'\n');
        bytes
    } else {
        b"Source postcondition established within restricted C03 pure-i64 profile.\nEvidence acceptance: not evaluated; no LLVM, artifact or final-executable assurance.\n".to_vec()
    };
    output
        .write_all(&bytes)
        .map_err(|_| "cannot write source evidence")?;
    Ok(0)
}

fn declared_adapter_gate(
    records: &[verification_evidence::Evidence],
    expected_adapter: Option<&str>,
) -> Result<bool, &'static str> {
    let Some(expected) = expected_adapter else {
        return Ok(false);
    };
    if expected.is_empty() {
        return Err("expected adapter id must not be empty");
    }
    if records
        .iter()
        .all(|record| record.provenance.adapter.id == expected)
    {
        Ok(true)
    } else {
        Err("declared producer does not match the expected adapter")
    }
}

fn check(
    input: Option<&Path>,
    stdin: bool,
    json: bool,
    expected_adapter: Option<&str>,
    output: &mut impl Write,
) -> Result<(), &'static str> {
    let bytes = match (input, stdin) {
        (Some(path), false) => super::input::read_regular(path, MAX_INPUT_BYTES as u64)?,
        (None, true) => super::input::read_stdin(MAX_INPUT_BYTES as u64)?,
        _ => return Err("choose exactly one evidence input"),
    };
    let record =
        verification_evidence::decode(&bytes).map_err(|_| "invalid evidence representation")?;
    let gated = declared_adapter_gate(std::slice::from_ref(&record), expected_adapter)?;
    let diagnostic = if json {
        let mut body = serde_json::json!({
            "schema_version": "mumei.evidence-representation-check/v1",
            "evidence_schema_version": SCHEMA_VERSION,
            "representation": "valid",
            "acceptance": "not_evaluated",
            "checks_not_performed": ["claim_truth", "file_identity", "freshness",
                "producer_authenticity", "semantic_correspondence", "artifact_semantics"],
        });
        if gated {
            body["checks_performed"] =
                serde_json::json!(["representation_gate", "declared_producer_identity"]);
        }
        let mut bytes = serde_json::to_vec_pretty(&body)
            .map_err(|_| "cannot encode representation diagnostic")?;
        bytes.push(b'\n');
        bytes
    } else if gated {
        format!(
            "{}Declared producer identity: every record names the expected adapter.\n",
            TEXT
        )
        .into_bytes()
    } else {
        TEXT.as_bytes().to_vec()
    };
    output
        .write_all(&diagnostic)
        .map_err(|_| "cannot write representation diagnostic")
}

fn check_list(
    input: Option<&Path>,
    stdin: bool,
    json: bool,
    expected_adapter: Option<&str>,
    output: &mut impl Write,
) -> Result<(), &'static str> {
    let bytes = match (input, stdin) {
        (Some(path), false) => super::input::read_regular(path, MAX_INPUT_BYTES as u64)?,
        (None, true) => super::input::read_stdin(MAX_INPUT_BYTES as u64)?,
        _ => return Err("choose exactly one evidence input"),
    };
    let records = verification_evidence::decode_list(&bytes)
        .map_err(|_| "invalid evidence representation")?;
    let gated = declared_adapter_gate(&records, expected_adapter)?;
    let diagnostic = if json {
        let mut body = serde_json::json!({
            "schema_version": "mumei.evidence-representation-check/v1",
            "evidence_schema_version": verification_evidence::LIST_SCHEMA_VERSION,
            "representation": "valid",
            "record_count": records.len(),
            "acceptance": "not_evaluated",
            "checks_not_performed": ["claim_truth", "file_identity", "freshness",
                "producer_authenticity", "semantic_correspondence", "artifact_semantics",
                "cross_record_consistency"],
        });
        if gated {
            body["checks_performed"] =
                serde_json::json!(["representation_gate", "declared_producer_identity"]);
        }
        let mut bytes = serde_json::to_vec_pretty(&body)
            .map_err(|_| "cannot encode representation diagnostic")?;
        bytes.push(b'\n');
        bytes
    } else {
        format!("Evidence list representation valid: {} record(s) each pass the v1 gate.\n\
Scope: representation only; no claim truth, file identity, freshness, producer authenticity, semantic correspondence, artifact semantics or cross-record consistency checked.\n\
{}{}\n",
            records.len(),
            if gated {
                "Declared producer identity: every record names the expected adapter.\n"
            } else {
                ""
            },
            "Evidence acceptance: not evaluated.")
        .into_bytes()
    };
    output
        .write_all(&diagnostic)
        .map_err(|_| "cannot write representation diagnostic")
}

fn check_consistency(
    input: Option<&Path>,
    stdin: bool,
    json: bool,
    expected_adapter: Option<&str>,
    output: &mut impl Write,
) -> Result<(), &'static str> {
    let bytes = match (input, stdin) {
        (Some(path), false) => super::input::read_regular(path, MAX_INPUT_BYTES as u64)?,
        (None, true) => super::input::read_stdin(MAX_INPUT_BYTES as u64)?,
        _ => return Err("choose exactly one evidence input"),
    };
    let records = verification_evidence::decode_list(&bytes)
        .map_err(|_| "invalid evidence representation")?;
    let gated = declared_adapter_gate(&records, expected_adapter)?;
    let report = verification_evidence::consistency_check(&records);
    let diagnostic = if json {
        let mut body = serde_json::json!({
            "schema_version": "mumei.evidence-consistency-check/v1",
            "evidence_schema_version": verification_evidence::LIST_SCHEMA_VERSION,
            "representation": "valid",
            "record_count": records.len(),
            "consistent": report.consistent,
            "conflict_count": report.conflict_count,
            "conflicts": report.conflicts,
            "acceptance": "not_evaluated",
            "checks_not_performed": ["claim_truth", "file_identity", "freshness",
                "producer_authenticity", "semantic_correspondence", "artifact_semantics",
                "pin_correctness"],
        });
        if gated {
            body["checks_performed"] = serde_json::json!([
                "representation_gate",
                "cross_record_pin_agreement",
                "declared_producer_identity"
            ]);
        }
        let mut bytes =
            serde_json::to_vec_pretty(&body).map_err(|_| "cannot encode consistency diagnostic")?;
        bytes.push(b'\n');
        bytes
    } else {
        format!(
            "Evidence list consistency: {} record(s), {}.\n\
Scope: syntactic identity-pin agreement only; no claim truth, file identity, freshness, producer authenticity, semantic correspondence, artifact semantics or pin correctness checked.\n\
{}Evidence acceptance: not evaluated.\n",
            records.len(),
            if report.consistent {
                "no conflicts".to_owned()
            } else {
                format!("{} conflict(s)", report.conflict_count)
            },
            if gated {
                "Declared producer identity: every record names the expected adapter.\n"
            } else {
                ""
            }
        )
        .into_bytes()
    };
    output
        .write_all(&diagnostic)
        .map_err(|_| "cannot write consistency diagnostic")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BrokenOutput;

    #[test]
    fn invalid_input_selection_cannot_read_or_write() {
        for input in [None, Some(Path::new("FAKE_PRIVATE_NONEXISTENT"))] {
            let stdin = input.is_some();
            for json in [false, true] {
                assert_eq!(
                    check(input, stdin, json, None, &mut BrokenOutput).unwrap_err(),
                    "choose exactly one evidence input"
                );
            }
        }
    }
    impl Write for BrokenOutput {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("FAKE_PRIVATE_OUTPUT_ERROR"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn source_recheck_only_writes_anonymous_diagnostics_on_consistency() {
        for json in [false, true] {
            for (outcome, code) in [
                (RecheckOutcome::Refuted, 1),
                (RecheckOutcome::Inconclusive, 3),
            ] {
                let mut output = Vec::new();
                assert_eq!(write_recheck(outcome, json, &mut output).unwrap(), code);
                assert!(output.is_empty());
            }
            assert_eq!(
                write_recheck(RecheckOutcome::Consistent, json, &mut BrokenOutput).unwrap_err(),
                "cannot write source recheck diagnostic"
            );
        }
    }

    #[test]
    fn source_acceptance_only_writes_anonymous_descriptor_on_acceptance() {
        for json in [false, true] {
            for (outcome, code) in [
                (AcceptOutcome::Rejected, 1),
                (AcceptOutcome::Inconclusive, 3),
            ] {
                let mut output = Vec::new();
                assert_eq!(
                    write_accept(outcome, json, b"record", &mut output).unwrap(),
                    code
                );
                assert!(output.is_empty());
            }
            assert_eq!(
                write_accept(AcceptOutcome::Accepted, json, b"record", &mut BrokenOutput)
                    .unwrap_err(),
                "cannot write source acceptance descriptor"
            );
            let mut output = Vec::new();
            assert_eq!(
                write_accept(AcceptOutcome::Accepted, json, b"record", &mut output).unwrap(),
                0
            );
            let text = String::from_utf8(output).unwrap();
            assert!(!text.contains("FAKE_PRIVATE"));
            if json {
                let diagnostic: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(
                    diagnostic["schema_version"],
                    "mumei.source-evidence-acceptance/v1"
                );
                assert_eq!(diagnostic["acceptance"], "evaluated");
                assert_eq!(diagnostic["accepted"], true);
                assert_eq!(
                    diagnostic["evidence_fingerprint"].as_str().unwrap().len(),
                    64
                );
            }
        }
    }

    #[test]
    fn source_non_success_and_writer_errors_never_expose_models_or_input() {
        let source = include_str!("../../tests/fixtures/semantic_contract_clamp.mm");
        for json in [false, true] {
            for (outcome, code) in [(Outcome::Refuted, 1), (Outcome::Inconclusive, 3)] {
                let mut output = Vec::new();
                assert_eq!(write_source(outcome, json, &mut output).unwrap(), code);
                assert!(output.is_empty());
            }
            let outcome = source_evidence::verify(source, Options::default()).unwrap();
            assert_eq!(
                write_source(outcome, json, &mut BrokenOutput).unwrap_err(),
                "cannot write source evidence"
            );
        }
    }

    #[test]
    fn invalid_inputs_never_write_text_or_json_and_output_errors_are_static() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("FAKE_PRIVATE_INPUT");
        std::fs::write(&path, b"FAKE_PRIVATE_MALFORMED").unwrap();
        for json in [false, true] {
            let mut output = Vec::new();
            let error = check(Some(&path), false, json, None, &mut output).unwrap_err();
            assert!(!error.contains("FAKE_PRIVATE"));
            assert!(output.is_empty());
        }
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/verification-evidence/synthetic_cases.json"
        ))
        .unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec(&fixtures["cases"][0]["evidence"]).unwrap(),
        )
        .unwrap();
        for json in [false, true] {
            assert_eq!(
                check(Some(&path), false, json, None, &mut BrokenOutput).unwrap_err(),
                "cannot write representation diagnostic"
            );
        }
    }

    #[test]
    fn declared_adapter_gate_matches_mismatches_and_stays_static() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("FAKE_PRIVATE_INPUT");
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/verification-evidence/synthetic_cases.json"
        ))
        .unwrap();
        let record = &fixtures["cases"][0]["evidence"];
        assert_eq!(record["provenance"]["adapter"]["id"], "synthetic-adapter");
        std::fs::write(&path, serde_json::to_vec(record).unwrap()).unwrap();

        for json in [false, true] {
            let mut output = Vec::new();
            check(
                Some(&path),
                false,
                json,
                Some("synthetic-adapter"),
                &mut output,
            )
            .unwrap();
            let text = String::from_utf8(output).unwrap();
            if json {
                let diagnostic: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(
                    diagnostic["checks_performed"],
                    serde_json::json!(["representation_gate", "declared_producer_identity"])
                );
                assert!(diagnostic["checks_not_performed"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("producer_authenticity")));
            } else {
                assert!(text.contains("Declared producer identity"));
            }

            let mut output = Vec::new();
            assert_eq!(
                check(
                    Some(&path),
                    false,
                    json,
                    Some("different-adapter"),
                    &mut output,
                )
                .unwrap_err(),
                "declared producer does not match the expected adapter"
            );
            assert!(output.is_empty());
            assert_eq!(
                check(Some(&path), false, json, Some(""), &mut output).unwrap_err(),
                "expected adapter id must not be empty"
            );

            let mut output = Vec::new();
            check(Some(&path), false, json, None, &mut output).unwrap();
            let text = String::from_utf8(output).unwrap();
            assert!(!text.contains("Declared producer identity"));
            if json {
                let diagnostic: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert!(diagnostic["checks_performed"].is_null());
            }
        }
    }
}
