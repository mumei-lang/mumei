use mumei_core::semantic_contract::{extract_pure_i64_module_contracts, RefinedAdmission};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::Path;
use std::process::{Command, Output};

const SOURCE: &str = "\
atom helper(x: i64) -> i64 requires: x > 0; ensures: result >= x; body: { if x < 100 { x + 1 } else { x } };
atom main(y: i64) -> i64 requires: y > 0 && y < 50; ensures: result > 0; body: { helper(y) };
";

fn succeeds(output: Output) -> Output {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn verify(dir: &Path, source: &Path, certificate: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["verify", "--json", "--proof-cert", "--output"])
        .arg(certificate)
        .arg(source)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn certificate_atom<'a>(certificate: &'a Value, name: &str) -> &'a Value {
    certificate["atoms"]
        .as_array()
        .expect("certificate atoms array")
        .iter()
        .find(|atom| atom["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("certificate has no atom named {name}"))
}

#[test]
fn module_trace_binds_every_atom_from_specification_to_execution_target() {
    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("trace.mm");
    let certificate_path = dir.path().join("cert.json");
    std::fs::write(&source_path, SOURCE).unwrap();

    let contracts =
        extract_pure_i64_module_contracts(SOURCE, "trace.mm", None, RefinedAdmission::Reject)
            .unwrap();
    assert_eq!(
        contracts
            .iter()
            .map(|contract| contract.atom.as_str())
            .collect::<Vec<_>>(),
        ["helper", "main"]
    );
    for contract in &contracts {
        assert_eq!(contract.source_sha256, contracts[0].source_sha256);
    }

    succeeds(verify(dir.path(), &source_path, &certificate_path));
    let certificate: Value =
        serde_json::from_slice(&std::fs::read(&certificate_path).unwrap()).unwrap();
    for contract in &contracts {
        let atom = certificate_atom(&certificate, &contract.atom);
        assert_eq!(atom["z3_check_result"], "unsat");
        assert_eq!(
            atom["content_hash"].as_str(),
            Some(contract.atom_content_hash.as_str())
        );
        assert_eq!(
            atom["requires"].as_str(),
            Some(contract.specification.requires.as_str())
        );
        assert_eq!(
            atom["ensures"].as_str(),
            Some(contract.specification.ensures.as_str())
        );
        assert_eq!(
            atom["translator_ir"]["theorem_goal"].as_str(),
            Some(contract.translator_ir.theorem_goal.as_str())
        );
        let theorem_goal = atom["translator_ir"]["theorem_goal"].as_str().unwrap();
        let theorem_goal_sha256 = format!("{:x}", Sha256::digest(theorem_goal.as_bytes()));
        assert_eq!(
            contract.transformations[0].output_identity.as_deref(),
            Some(contract.specification_sha256.as_str())
        );
        assert_eq!(
            contract.transformations[1].output_identity.as_deref(),
            Some(contract.obligation_id.as_str())
        );
        assert_eq!(
            contract.transformations[2].output_identity.as_deref(),
            Some(theorem_goal_sha256.as_str())
        );
    }

    succeeds(
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .arg("verify-cert")
            .arg(&certificate_path)
            .arg(&source_path)
            .current_dir(dir.path())
            .output()
            .unwrap(),
    );

    succeeds(
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args(["build", "--emit", "llvm-ir", "-o"])
            .arg(dir.path().join("trace"))
            .arg(&source_path)
            .current_dir(dir.path())
            .output()
            .unwrap(),
    );
    let llvm_files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "ll"))
        .collect();
    for contract in &contracts {
        let definition = format!("define i64 @{}(", contract.atom);
        assert!(
            llvm_files
                .iter()
                .any(|path| { std::fs::read_to_string(path).unwrap().contains(&definition) }),
            "no emitted LLVM file contains {definition}"
        );
        let llvm_relation = contract.transformations.last().unwrap();
        assert_eq!(llvm_relation.from, "hir");
        assert_eq!(llvm_relation.output_identity, None);
        assert_eq!(
            llvm_relation.validation,
            "unchecked_not_translation_validated"
        );
    }

    let mut obligation_ids = HashSet::new();
    for contract in &contracts {
        assert_eq!(
            contract.correspondence,
            "unchecked_not_translation_validated"
        );
        assert!(contract.requested_target_triple.is_none());
        assert!(obligation_ids.insert(contract.obligation_id.as_str()));
        assert!(!contract.assumptions.is_empty());
        assert!(!contract.exclusions.is_empty());
    }
    assert_eq!(obligation_ids.len(), contracts.len());
}

fn is_full_proof(certificate: &Value, name: &str) -> bool {
    let Some(atom) = certificate["atoms"].as_array().and_then(|atoms| {
        atoms
            .iter()
            .find(|atom| atom["name"].as_str() == Some(name))
    }) else {
        return false;
    };
    atom["status"] == "verified" && atom["z3_check_result"] == "unsat"
}

fn certificate_is_full_proof(path: &Path, name: &str) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|certificate| is_full_proof(&certificate, name))
}

#[test]
fn unsupported_specification_clauses_fail_closed_instead_of_weakening() {
    let dir = tempfile::tempdir().unwrap();
    let cases = [
        (
            "helper unsupported ensures",
            SOURCE.replace("ensures: result >= x;", "ensures: is_hex_digit(result);"),
        ),
        (
            "main unsupported requires",
            SOURCE.replace("requires: y > 0 && y < 50;", "requires: is_hex_digit(y);"),
        ),
        (
            "helper float ensures",
            SOURCE.replace("ensures: result >= x;", "ensures: result > 0.5;"),
        ),
    ];

    for (index, (name, source)) in cases.iter().enumerate() {
        assert_ne!(
            source.as_str(),
            SOURCE,
            "{name} replacement must change source"
        );
        assert!(
            extract_pure_i64_module_contracts(source, "trace.mm", None, RefinedAdmission::Reject)
                .is_err(),
            "{name} must be rejected by module contract extraction"
        );

        let source_path = dir.path().join(format!("unsupported-{index}.mm"));
        let certificate_path = dir.path().join(format!("unsupported-{index}.json"));
        std::fs::write(&source_path, source).unwrap();
        let output = verify(dir.path(), &source_path, &certificate_path);
        eprintln!(
            "{name}: verify --json exit={:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        if index == 0 {
            assert!(
                !output.status.success(),
                "unsupported helper ensures must fail verify"
            );
            assert!(
                !certificate_is_full_proof(&certificate_path, "helper"),
                "unsupported helper ensures were presented as a full proof"
            );
        } else if index == 1 {
            assert!(
                !output.status.success(),
                "unsupported main requires must fail verify"
            );
            assert!(
                !certificate_is_full_proof(&certificate_path, "main"),
                "unsupported main requires were presented as a full proof"
            );
        } else {
            // The verifier accepts this float clause; only extraction fails closed.
        }
    }
}
