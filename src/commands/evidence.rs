use crate::cli::EvidenceCommand;
use mumei_core::source_evidence::{self, Options, Outcome, RecheckOutcome};
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
        EvidenceCommand::Check { input, stdin, json } => (
            check(input.as_deref().map(Path::new), stdin, json, &mut output).map(|()| 0),
            "Evidence representation check failed",
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

fn check(
    input: Option<&Path>,
    stdin: bool,
    json: bool,
    output: &mut impl Write,
) -> Result<(), &'static str> {
    let bytes = match (input, stdin) {
        (Some(path), false) => super::input::read_regular(path, MAX_INPUT_BYTES as u64)?,
        (None, true) => super::input::read_stdin(MAX_INPUT_BYTES as u64)?,
        _ => return Err("choose exactly one evidence input"),
    };
    verification_evidence::decode(&bytes).map_err(|_| "invalid evidence representation")?;
    let diagnostic = if json {
        let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "mumei.evidence-representation-check/v1",
            "evidence_schema_version": SCHEMA_VERSION,
            "representation": "valid",
            "acceptance": "not_evaluated",
            "checks_not_performed": ["claim_truth", "file_identity", "freshness",
                "producer_authenticity", "semantic_correspondence", "artifact_semantics"],
        }))
        .map_err(|_| "cannot encode representation diagnostic")?;
        bytes.push(b'\n');
        bytes
    } else {
        TEXT.as_bytes().to_vec()
    };
    output
        .write_all(&diagnostic)
        .map_err(|_| "cannot write representation diagnostic")
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
                    check(input, stdin, json, &mut BrokenOutput).unwrap_err(),
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
            let error = check(Some(&path), false, json, &mut output).unwrap_err();
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
                check(Some(&path), false, json, &mut BrokenOutput).unwrap_err(),
                "cannot write representation diagnostic"
            );
        }
    }
}
