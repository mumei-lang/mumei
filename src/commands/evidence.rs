use crate::cli::EvidenceCommand;
use mumei_core::evidence_acceptance::{self, Verdict};
use mumei_core::producer_signature::{self, PRODUCER_SIGNATURE_SCHEMA};
use mumei_core::source_evidence::{self, AcceptOutcome, Options, Outcome, RecheckOutcome};
use mumei_core::verification_evidence::{self, MAX_INPUT_BYTES, SCHEMA_VERSION};
use sha2::{Digest, Sha256};
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
            produced_at,
        } => (
            source_options(&solver_timeout, &solver_resource_limit, produced_at).and_then(
                |options| {
                    recheck_source(
                        Path::new(&input),
                        Path::new(&source),
                        json,
                        options,
                        &mut output,
                    )
                },
            ),
            "Source evidence recheck failed",
        ),
        EvidenceCommand::AcceptSource {
            input,
            source,
            json,
            solver_timeout,
            solver_resource_limit,
            produced_at,
        } => (
            source_options(&solver_timeout, &solver_resource_limit, produced_at).and_then(
                |options| {
                    accept_source(
                        Path::new(&input),
                        Path::new(&source),
                        json,
                        options,
                        &mut output,
                    )
                },
            ),
            "Source evidence acceptance failed",
        ),
        EvidenceCommand::Accept {
            input,
            stdin,
            requirement,
            requirement_list,
            json,
        } => (
            accept(
                input.as_deref().map(Path::new),
                stdin,
                json,
                requirement.as_deref().map(Path::new),
                requirement_list.as_deref().map(Path::new),
                &mut output,
            ),
            if requirement_list.is_some() {
                "Evidence requirement list acceptance evaluation failed"
            } else {
                "Evidence acceptance evaluation failed"
            },
        ),
        EvidenceCommand::AcceptList {
            input,
            stdin,
            requirement,
            requirement_list,
            json,
        } => (
            accept_list(
                input.as_deref().map(Path::new),
                stdin,
                json,
                requirement.as_deref().map(Path::new),
                requirement_list.as_deref().map(Path::new),
                &mut output,
            ),
            if requirement_list.is_some() {
                "Evidence requirement list acceptance evaluation failed"
            } else {
                "Evidence list acceptance evaluation failed"
            },
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
        EvidenceCommand::Sign {
            input,
            stdin,
            json,
            key_file,
            producer_key_id,
            produced_at,
        } => (
            sign(
                input.as_deref().map(Path::new),
                stdin,
                json,
                Path::new(&key_file),
                &producer_key_id,
                produced_at.as_deref(),
                &mut output,
            )
            .map(|()| 0),
            "Producer signature failed",
        ),
        EvidenceCommand::Keygen { secret_file } => (
            keygen(Path::new(&secret_file), &mut output).map(|()| 0),
            "Producer key generation failed",
        ),
        EvidenceCommand::VerifySource {
            input,
            json,
            solver_timeout,
            solver_resource_limit,
            produced_at,
        } => (
            source_options(&solver_timeout, &solver_resource_limit, produced_at)
                .and_then(|options| verify_source(Path::new(&input), json, options, &mut output)),
            "Source evidence check failed",
        ),
    };
    match result {
        Ok(0) => (),
        Ok(code) => {
            let message = match (prefix, code) {
                ("Evidence acceptance evaluation failed", 3) => {
                    "Evidence acceptance inconclusive under the declared requirement; nothing accepted"
                }
                ("Evidence acceptance evaluation failed", _) => {
                    "Evidence rejected under the declared requirement; nothing accepted"
                }
                ("Evidence list acceptance evaluation failed", 3) => {
                    "Evidence list acceptance inconclusive under the declared requirement; no record accepted"
                }
                ("Evidence list acceptance evaluation failed", _) => {
                    "Evidence rejected under the declared requirement; no record accepted"
                }
                ("Evidence requirement list acceptance evaluation failed", 3) => {
                    "Evidence requirement list acceptance inconclusive; not every declared requirement accepted"
                }
                ("Evidence requirement list acceptance evaluation failed", _) => {
                    "Evidence requirement list rejected; a declared requirement was rejected"
                }
                (_, 3) => {
                    "Source query inconclusive within bounded profile; no Evidence emitted"
                }
                (_, _) => {
                    "Source postcondition refuted within restricted profile; no Evidence emitted"
                }
            };
            eprintln!("{message}");
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
    write_recheck(outcome, json, &saved, output)
}

fn write_recheck(
    outcome: RecheckOutcome,
    json: bool,
    saved: &[u8],
    output: &mut impl Write,
) -> Result<i32, &'static str> {
    match outcome {
        RecheckOutcome::Refuted => return Ok(1),
        RecheckOutcome::Inconclusive => return Ok(3),
        RecheckOutcome::Consistent => (),
    }
    let bytes = if json {
        use sha2::{Digest, Sha256};
        let evidence_fingerprint = format!("{:x}", Sha256::digest(saved));
        let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "mumei.source-evidence-recheck/v1",
            "evidence_schema_version": SCHEMA_VERSION,
            "evidence_fingerprint": evidence_fingerprint,
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

fn source_options(
    timeout: &str,
    resource_limit: &str,
    produced_at: Option<String>,
) -> Result<Options, &'static str> {
    if let Some(value) = &produced_at {
        if !verification_evidence::is_rfc3339_timestamp(value) {
            return Err("--produced-at must be an RFC 3339 timestamp");
        }
    }
    Ok(Options {
        timeout_ms: timeout
            .parse()
            .map_err(|_| "invalid bounded source-query budget")?,
        resource_limit: resource_limit
            .parse()
            .map_err(|_| "invalid bounded source-query budget")?,
        produced_at,
    })
}

fn sign(
    input: Option<&Path>,
    stdin: bool,
    json: bool,
    key_file: &Path,
    producer_key_id: &str,
    produced_at: Option<&str>,
    output: &mut impl Write,
) -> Result<(), &'static str> {
    let record = match (input, stdin) {
        (Some(path), false) => super::input::read_regular(path, MAX_INPUT_BYTES as u64)?,
        (None, true) => super::input::read_stdin(MAX_INPUT_BYTES as u64)?,
        _ => return Err("choose exactly one evidence input"),
    };
    let key_bytes = super::input::read_regular(key_file, 64)?;
    let key = producer_signature::decode_secret_key_bytes(key_bytes)
        .map_err(|_| "invalid producer key material")?;
    let envelope =
        producer_signature::sign(&record, &key, producer_key_id, produced_at).map_err(|error| {
            match error {
                producer_signature::SignError::InvalidProducerKeyId => {
                    "producer key id must not be empty"
                }
                producer_signature::SignError::InvalidKeyMaterial => {
                    "invalid producer key material"
                }
                producer_signature::SignError::InvalidProducedAt => {
                    "--produced-at must be an RFC 3339 timestamp"
                }
            }
        })?;
    debug_assert_eq!(envelope.schema_version, PRODUCER_SIGNATURE_SCHEMA);
    let bytes = if json {
        let mut bytes = serde_json::to_vec_pretty(&envelope)
            .map_err(|_| "cannot encode producer signature envelope")?;
        bytes.push(b'\n');
        bytes
    } else {
        format!(
            "Producer signature envelope issued for subject digest {}.\n\
Scope: the detached signature covers the subject digest only; the producer key id is a\n\
declared label the consumer pins separately, not signed or attested;\n\
no key or producer authenticity, freshness, revocation, claim truth or acceptance evaluated.\n",
            envelope.subject_digest
        )
        .into_bytes()
    };
    output
        .write_all(&bytes)
        .map_err(|_| "cannot write producer signature envelope")
}

fn keygen(secret_file: &Path, output: &mut impl Write) -> Result<(), &'static str> {
    let (mut secret_hex, public_hex) = producer_signature::generate_keypair();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = options
        .open(secret_file)
        .map_err(|_| "cannot create producer secret key file")
        .and_then(|mut file| {
            file.write_all(secret_hex.as_bytes())
                .map_err(|_| "cannot write producer secret key file")
        });
    zeroize::Zeroize::zeroize(&mut secret_hex);
    result?;
    #[cfg(unix)]
    let permissions_note = "";
    #[cfg(not(unix))]
    let permissions_note =
        "The file inherits the directory's access policy on this platform — restrict it to the owner yourself.\n";
    output
        .write_all(
            format!(
                "Producer keypair generated; the secret key was written to the declared file only and is never printed.\n\
Public key: {public_hex}\n\
{permissions_note}\
Scope: pin the public key over a separately trusted channel; no key authenticity, rotation or revocation evaluated.\n"
            )
            .as_bytes(),
        )
        .map_err(|_| "cannot write producer public key")
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

fn accept(
    input: Option<&Path>,
    stdin: bool,
    json: bool,
    requirement: Option<&Path>,
    requirement_list: Option<&Path>,
    output: &mut impl Write,
) -> Result<i32, &'static str> {
    let record_bytes = match (input, stdin) {
        (Some(path), false) => super::input::read_regular(path, MAX_INPUT_BYTES as u64)?,
        (None, true) => super::input::read_stdin(MAX_INPUT_BYTES as u64)?,
        _ => return Err("choose exactly one evidence input"),
    };
    let record = verification_evidence::decode(&record_bytes)
        .map_err(|_| "invalid evidence representation")?;
    match (requirement, requirement_list) {
        (Some(requirement), None) => {
            let requirement_bytes =
                super::input::read_regular(requirement, MAX_INPUT_BYTES as u64)?;
            let requirement = evidence_acceptance::decode_requirement(&requirement_bytes)
                .map_err(|_| "invalid evidence requirement representation")?;
            accept_single(
                &record,
                &record_bytes,
                &requirement,
                &requirement_bytes,
                json,
                output,
            )
        }
        (None, Some(requirement_list)) => {
            let requirement_list_bytes =
                super::input::read_regular(requirement_list, MAX_INPUT_BYTES as u64)?;
            let records = [record];
            let consistency = verification_evidence::consistency_check(&records);
            let per_requirement =
                eval_requirement_list(&requirement_list_bytes, &records, &consistency)?;
            let verdict = conjunctive_verdict(&per_requirement);
            accept_list_conjunctive(
                verdict,
                &per_requirement,
                &consistency,
                &requirement_list_bytes,
                ConjunctiveEvidence {
                    pin_name: "evidence_fingerprint",
                    fingerprint: format!("{:x}", Sha256::digest(record_bytes)),
                    schema_version: SCHEMA_VERSION,
                },
                json,
                output,
            )
        }
        _ => Err("choose exactly one requirement declaration"),
    }
}

fn accept_single(
    record: &verification_evidence::Evidence,
    record_bytes: &[u8],
    requirement: &evidence_acceptance::Requirement,
    requirement_bytes: &[u8],
    json: bool,
    output: &mut impl Write,
) -> Result<i32, &'static str> {
    let evaluation = evidence_acceptance::evaluate(requirement, record);
    let code = match evaluation.verdict {
        Verdict::Accepted => 0,
        Verdict::Rejected => 1,
        Verdict::Inconclusive => 3,
    };
    let descriptor = if json {
        use sha2::{Digest, Sha256};
        let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": evidence_acceptance::ACCEPTANCE_SCHEMA,
            "evidence_schema_version": SCHEMA_VERSION,
            "requirement_schema_version": evidence_acceptance::REQUIREMENT_SCHEMA,
            "requirement_fingerprint": format!("{:x}", Sha256::digest(requirement_bytes)),
            "evidence_fingerprint": format!("{:x}", Sha256::digest(record_bytes)),
            "acceptance": "evaluated",
            "verdict": evaluation.verdict.as_str(),
            "checks": evaluation
                .checks
                .iter()
                .map(|check| serde_json::json!({"name": check.name, "outcome": check.outcome}))
                .collect::<Vec<_>>(),
            "checks_performed": ["representation_gate", "declared_requirement_normalization"],
            "checks_not_performed": ["claim_truth", "producer_authenticity",
                "historical_execution", "record_freshness",
                "independent_model_correspondence", "artifact_semantics",
                "tool_execution", "grade_promotion"],
        }))
        .map_err(|_| "cannot encode evidence acceptance descriptor")?;
        bytes.push(b'\n');
        bytes
    } else {
        let failed = evaluation
            .checks
            .iter()
            .filter(|check| check.outcome == "failed")
            .count();
        let open = evaluation
            .checks
            .iter()
            .filter(|check| check.outcome == "indeterminate")
            .count();
        format!(
            "Evidence acceptance: {} ({} check(s) failed, {} indeterminate).\n\
Scope: declared-requirement normalization over the record's representation only; no claim truth, producer authenticity, freshness, independent correspondence, artifact semantics, tool execution or grade promotion checked.\n",
            evaluation.verdict.as_str(),
            failed,
            open,
        )
        .into_bytes()
    };
    output
        .write_all(&descriptor)
        .map_err(|_| "cannot write evidence acceptance descriptor")?;
    Ok(code)
}

fn accept_list(
    input: Option<&Path>,
    stdin: bool,
    json: bool,
    requirement: Option<&Path>,
    requirement_list: Option<&Path>,
    output: &mut impl Write,
) -> Result<i32, &'static str> {
    let list_bytes = match (input, stdin) {
        (Some(path), false) => super::input::read_regular(path, MAX_INPUT_BYTES as u64)?,
        (None, true) => super::input::read_stdin(MAX_INPUT_BYTES as u64)?,
        _ => return Err("choose exactly one evidence input"),
    };
    let records = verification_evidence::decode_list(&list_bytes)
        .map_err(|_| "invalid evidence representation")?;
    // Consumer-side arbitration over the closed list: one accepting
    // record satisfies the requirement — but only when the list's
    // identity pins agree: an affirmative acceptance over a
    // contradictory set is never a silent pass. Otherwise an
    // affirmative rejection on any record rejects, and a list with no
    // verdict either way stays inconclusive.
    let consistency = verification_evidence::consistency_check(&records);
    let arbitrate = |evaluations: &[evidence_acceptance::Evaluation]| {
        if evaluations.iter().any(|ev| ev.verdict == Verdict::Accepted) {
            if consistency.consistent {
                Verdict::Accepted
            } else {
                Verdict::Inconclusive
            }
        } else if evaluations.iter().any(|ev| ev.verdict == Verdict::Rejected) {
            Verdict::Rejected
        } else {
            Verdict::Inconclusive
        }
    };
    match (requirement, requirement_list) {
        (Some(requirement), None) => {
            let requirement_bytes =
                super::input::read_regular(requirement, MAX_INPUT_BYTES as u64)?;
            let requirement = evidence_acceptance::decode_requirement(&requirement_bytes)
                .map_err(|_| "invalid evidence requirement representation")?;
            let evaluations: Vec<evidence_acceptance::Evaluation> = records
                .iter()
                .map(|record| evidence_acceptance::evaluate(&requirement, record))
                .collect();
            let verdict = arbitrate(&evaluations);
            accept_list_single(
                verdict,
                &evaluations,
                &consistency,
                &requirement_bytes,
                &list_bytes,
                json,
                output,
            )
        }
        (None, Some(requirement_list)) => {
            let requirement_list_bytes =
                super::input::read_regular(requirement_list, MAX_INPUT_BYTES as u64)?;
            let per_requirement =
                eval_requirement_list(&requirement_list_bytes, &records, &consistency)?;
            let verdict = conjunctive_verdict(&per_requirement);
            accept_list_conjunctive(
                verdict,
                &per_requirement,
                &consistency,
                &requirement_list_bytes,
                ConjunctiveEvidence {
                    pin_name: "list_fingerprint",
                    fingerprint: format!("{:x}", Sha256::digest(list_bytes)),
                    schema_version: verification_evidence::LIST_SCHEMA_VERSION,
                },
                json,
                output,
            )
        }
        _ => Err("choose exactly one requirement declaration"),
    }
}

#[derive(serde::Deserialize)]
struct RequirementListEnvelope {
    requirements: Vec<Box<serde_json::value::RawValue>>,
}

/// Which field the conjunctive descriptor pins the evaluated evidence
/// under — a single record (`evidence_fingerprint`) or a list envelope
/// (`list_fingerprint`), with its declared schema version.
struct ConjunctiveEvidence {
    pin_name: &'static str,
    fingerprint: String,
    schema_version: &'static str,
}

/// One requirement's conjunctive outcome: its arbitrated verdict, its
/// per-record evaluations and its raw-bytes fingerprint.
type RequirementOutcome = (Verdict, Vec<evidence_acceptance::Evaluation>, String);

/// Decode a declared requirement list and arbitrate every requirement
/// against `records`, returning each requirement's outcome.
fn eval_requirement_list(
    requirement_list_bytes: &[u8],
    records: &[verification_evidence::Evidence],
    consistency: &verification_evidence::ConsistencyReport,
) -> Result<Vec<RequirementOutcome>, &'static str> {
    let envelope: Vec<Box<serde_json::value::RawValue>> =
        serde_json::from_slice::<RequirementListEnvelope>(requirement_list_bytes)
            .map_err(|_| "invalid evidence requirement representation")?
            .requirements;
    let fingerprints: Vec<String> = envelope
        .iter()
        .map(|element| format!("{:x}", Sha256::digest(element.get().as_bytes())))
        .collect();
    let requirements = evidence_acceptance::decode_requirement_list(requirement_list_bytes)
        .map_err(|_| "invalid evidence requirement representation")?;
    let arbitrate = |evaluations: &[evidence_acceptance::Evaluation]| {
        if evaluations.iter().any(|ev| ev.verdict == Verdict::Accepted) {
            if consistency.consistent {
                Verdict::Accepted
            } else {
                Verdict::Inconclusive
            }
        } else if evaluations.iter().any(|ev| ev.verdict == Verdict::Rejected) {
            Verdict::Rejected
        } else {
            Verdict::Inconclusive
        }
    };
    Ok(requirements
        .iter()
        .zip(fingerprints)
        .map(|(requirement, fingerprint)| {
            let evaluations: Vec<evidence_acceptance::Evaluation> = records
                .iter()
                .map(|record| evidence_acceptance::evaluate(requirement, record))
                .collect();
            (arbitrate(&evaluations), evaluations, fingerprint)
        })
        .collect())
}

/// The declared requirement list is conjunctive: every requirement must
/// arbitrate to accepted — an affirmative rejection on any requirement
/// rejects the whole list, and anything short of all-accepted stays
/// inconclusive.
fn conjunctive_verdict(per_requirement: &[RequirementOutcome]) -> Verdict {
    if per_requirement
        .iter()
        .all(|(verdict, ..)| *verdict == Verdict::Accepted)
    {
        Verdict::Accepted
    } else if per_requirement
        .iter()
        .any(|(verdict, ..)| *verdict == Verdict::Rejected)
    {
        Verdict::Rejected
    } else {
        Verdict::Inconclusive
    }
}

fn accept_list_single(
    verdict: Verdict,
    evaluations: &[evidence_acceptance::Evaluation],
    consistency: &verification_evidence::ConsistencyReport,
    requirement_bytes: &[u8],
    list_bytes: &[u8],
    json: bool,
    output: &mut impl Write,
) -> Result<i32, &'static str> {
    let code = match verdict {
        Verdict::Accepted => 0,
        Verdict::Rejected => 1,
        Verdict::Inconclusive => 3,
    };
    let descriptor = if json {
        use sha2::{Digest, Sha256};
        let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "mumei.evidence-list-acceptance/v1",
            "evidence_schema_version": verification_evidence::LIST_SCHEMA_VERSION,
            "requirement_schema_version": evidence_acceptance::REQUIREMENT_SCHEMA,
            "requirement_fingerprint": format!("{:x}", Sha256::digest(requirement_bytes)),
            "list_fingerprint": format!("{:x}", Sha256::digest(list_bytes)),
            "acceptance": "evaluated",
            "verdict": verdict.as_str(),
            "consistent": consistency.consistent,
            "conflict_count": consistency.conflict_count,
            "conflicts": consistency.conflicts,
            "records_evaluated": evaluations.len(),
            "records": evaluations
                .iter()
                .enumerate()
                .map(|(index, evaluation)| serde_json::json!({
                    "index": index,
                    "verdict": evaluation.verdict.as_str(),
                    "checks": evaluation
                        .checks
                        .iter()
                        .map(|check| serde_json::json!({"name": check.name, "outcome": check.outcome}))
                        .collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>(),
            "checks_performed": ["representation_gate",
                "declared_requirement_normalization",
                "cross_record_pin_agreement"],
            "checks_not_performed": ["claim_truth", "producer_authenticity",
                "historical_execution", "record_freshness",
                "independent_model_correspondence", "artifact_semantics",
                "tool_execution", "grade_promotion"],
        }))
        .map_err(|_| "cannot encode evidence acceptance descriptor")?;
        bytes.push(b'\n');
        bytes
    } else {
        let accepted = evaluations
            .iter()
            .filter(|ev| ev.verdict == Verdict::Accepted)
            .count();
        let rejected = evaluations
            .iter()
            .filter(|ev| ev.verdict == Verdict::Rejected)
            .count();
        format!(
            "Evidence list acceptance: {} ({} record(s): {} accepted, {} rejected, {} inconclusive; {}).\n\
Scope: declared-requirement normalization over each record's representation plus cross-record pin agreement; no claim truth, producer authenticity, freshness, independent correspondence, artifact semantics, tool execution or grade promotion checked.\n",
            verdict.as_str(),
            evaluations.len(),
            accepted,
            rejected,
            evaluations.len() - accepted - rejected,
            if consistency.consistent {
                "no consistency conflicts".to_owned()
            } else {
                format!("{} consistency conflict(s)", consistency.conflict_count)
            },
        )
        .into_bytes()
    };
    output
        .write_all(&descriptor)
        .map_err(|_| "cannot write evidence acceptance descriptor")?;
    Ok(code)
}

fn accept_list_conjunctive(
    verdict: Verdict,
    per_requirement: &[RequirementOutcome],
    consistency: &verification_evidence::ConsistencyReport,
    requirement_list_bytes: &[u8],
    evidence: ConjunctiveEvidence,
    json: bool,
    output: &mut impl Write,
) -> Result<i32, &'static str> {
    let code = match verdict {
        Verdict::Accepted => 0,
        Verdict::Rejected => 1,
        Verdict::Inconclusive => 3,
    };
    let descriptor = if json {
        let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": "mumei.evidence-requirement-list-acceptance/v1",
            "evidence_schema_version": evidence.schema_version,
            "requirement_schema_version": evidence_acceptance::REQUIREMENT_LIST_SCHEMA,
            "requirement_list_fingerprint": format!("{:x}", Sha256::digest(requirement_list_bytes)),
            evidence.pin_name: evidence.fingerprint,
            "acceptance": "evaluated",
            "verdict": verdict.as_str(),
            "consistent": consistency.consistent,
            "conflict_count": consistency.conflict_count,
            "conflicts": consistency.conflicts,
            "requirements_evaluated": per_requirement.len(),
            "records_evaluated": per_requirement
                .first()
                .map(|(_, evaluations, _)| evaluations.len())
                .unwrap_or(0),
            "requirements": per_requirement
                .iter()
                .enumerate()
                .map(|(index, (verdict, evaluations, fingerprint))| serde_json::json!({
                    "index": index,
                    "requirement_fingerprint": fingerprint,
                    "verdict": verdict.as_str(),
                    "records": evaluations
                        .iter()
                        .enumerate()
                        .map(|(index, evaluation)| serde_json::json!({
                            "index": index,
                            "verdict": evaluation.verdict.as_str(),
                            "checks": evaluation
                                .checks
                                .iter()
                                .map(|check| serde_json::json!({"name": check.name, "outcome": check.outcome}))
                                .collect::<Vec<_>>(),
                        }))
                        .collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>(),
            "checks_performed": ["representation_gate",
                "declared_requirement_normalization",
                "cross_record_pin_agreement",
                "conjunctive_requirement_fold"],
            "checks_not_performed": ["claim_truth", "producer_authenticity",
                "historical_execution", "record_freshness",
                "independent_model_correspondence", "artifact_semantics",
                "tool_execution", "grade_promotion"],
        }))
        .map_err(|_| "cannot encode evidence acceptance descriptor")?;
        bytes.push(b'\n');
        bytes
    } else {
        let accepted = per_requirement
            .iter()
            .filter(|(verdict, ..)| *verdict == Verdict::Accepted)
            .count();
        let rejected = per_requirement
            .iter()
            .filter(|(verdict, ..)| *verdict == Verdict::Rejected)
            .count();
        format!(
            "Evidence requirement list acceptance: {} ({} requirement(s): {} accepted, {} rejected, {} inconclusive; {}).\n\
Scope: declared-requirement normalization over each record's representation plus cross-record pin agreement and a conjunctive requirement fold; no claim truth, producer authenticity, freshness, independent correspondence, artifact semantics, tool execution or grade promotion checked.\n",
            verdict.as_str(),
            per_requirement.len(),
            accepted,
            rejected,
            per_requirement.len() - accepted - rejected,
            if consistency.consistent {
                "no consistency conflicts".to_owned()
            } else {
                format!("{} consistency conflict(s)", consistency.conflict_count)
            },
        )
        .into_bytes()
    };
    output
        .write_all(&descriptor)
        .map_err(|_| "cannot write evidence acceptance descriptor")?;
    Ok(code)
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
                assert_eq!(
                    write_recheck(outcome, json, b"record", &mut output).unwrap(),
                    code
                );
                assert!(output.is_empty());
            }
            assert_eq!(
                write_recheck(
                    RecheckOutcome::Consistent,
                    json,
                    b"record",
                    &mut BrokenOutput
                )
                .unwrap_err(),
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
