use crate::cli::EvidenceCommand;
use mumei_core::verification_evidence::{self, MAX_INPUT_BYTES, SCHEMA_VERSION};
use std::{io::Write, path::Path};

const TEXT: &str = "Evidence v1 representation valid.\n\
Scope: representation only; no claim truth, file identity, freshness, producer authenticity, semantic correspondence or artifact semantics checked.\n\
Evidence acceptance: not evaluated.\n\
No verifier or artifact executed; no grade promotion or final-executable assurance.\n";

pub(crate) fn run(command: EvidenceCommand) {
    let EvidenceCommand::Check { input, json } = command;
    if let Err(error) = check(Path::new(&input), json, &mut std::io::stdout().lock()) {
        eprintln!("Evidence representation check failed: {error}");
        std::process::exit(1);
    }
}

fn check(path: &Path, json: bool, output: &mut impl Write) -> Result<(), &'static str> {
    let bytes = super::input::read_regular(path, MAX_INPUT_BYTES as u64)?;
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

    #[test]
    fn invalid_inputs_never_write_text_or_json_and_output_errors_are_static() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("FAKE_PRIVATE_INPUT");
        std::fs::write(&path, b"FAKE_PRIVATE_MALFORMED").unwrap();
        for json in [false, true] {
            let mut output = Vec::new();
            let error = check(&path, json, &mut output).unwrap_err();
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
        struct BrokenOutput;
        impl Write for BrokenOutput {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("FAKE_PRIVATE_OUTPUT_ERROR"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for json in [false, true] {
            assert_eq!(
                check(&path, json, &mut BrokenOutput).unwrap_err(),
                "cannot write representation diagnostic"
            );
        }
    }
}
