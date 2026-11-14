use mumei_core::semantic_contract::extract_pure_i64_contract;
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

const SOURCE: &str = include_str!("fixtures/semantic_contract_clamp.mm");

fn succeeds(output: Output) -> Output {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn native_observation(ir: &Path, driver: &Path) -> Output {
    let object = ir.with_extension("o");
    let binary = ir.with_extension("native");
    mumei_emit_llvm::codegen::compile_llvm_ir_to_object(ir, &object).unwrap();
    succeeds(
        Command::new("cc")
            .arg(driver)
            .arg(&object)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap(),
    );
    Command::new(binary).output().unwrap()
}

fn rejects_postcondition(source: &Path) {
    let certificate = source.with_extension("proof.json");
    let output = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .args(["verify", "--json", "--proof-cert", "--output"])
        .arg(&certificate)
        .arg(source)
        .current_dir(source.parent().unwrap())
        .output()
        .unwrap();
    assert!(!output.status.success(), "a false contract must not verify");
    let cert: Value = serde_json::from_slice(&std::fs::read(certificate).unwrap()).unwrap();
    assert_eq!(cert["atoms"].as_array().unwrap().len(), 1);
    assert_eq!(
        cert["atoms"][0]["z3_check_result"], "sat",
        "parse errors, timeout, unknown and tool errors are not killed negative controls"
    );
}

#[test]
fn scalar_contract_connects_real_z3_certificate_and_llvm_generation_without_promotion() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("clamp.mm");
    let certificate = dir.path().join("clamp.proof.json");
    std::fs::write(&source, SOURCE).unwrap();
    let contract = extract_pure_i64_contract(SOURCE, "clamp.mm", None, false).unwrap();

    succeeds(
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args(["verify", "--json", "--proof-cert", "--output"])
            .arg(&certificate)
            .arg(&source)
            .current_dir(dir.path())
            .output()
            .unwrap(),
    );
    let cert: Value = serde_json::from_slice(&std::fs::read(&certificate).unwrap()).unwrap();
    assert_eq!(cert["atoms"].as_array().unwrap().len(), 1);
    assert_eq!(cert["atoms"][0]["z3_check_result"], "unsat");
    assert_eq!(cert["atoms"][0]["content_hash"], contract.atom_content_hash);
    assert_eq!(
        cert["atoms"][0]["requires"],
        contract.specification.requires
    );
    assert_eq!(cert["atoms"][0]["ensures"], contract.specification.ensures);
    assert_eq!(
        cert["atoms"][0]["translator_ir"]["theorem_goal"],
        contract.translator_ir.theorem_goal
    );

    succeeds(
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .arg("verify-cert")
            .arg(&certificate)
            .arg(&source)
            .current_dir(dir.path())
            .output()
            .unwrap(),
    );

    succeeds(
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .args(["build", "--emit", "llvm-ir", "-o"])
            .arg(dir.path().join("slice"))
            .arg(&source)
            .current_dir(dir.path())
            .output()
            .unwrap(),
    );
    let llvm = std::fs::read_to_string(dir.path().join("slice_clamp_nonnegative.ll")).unwrap();
    assert!(llvm.contains("define i64 @clamp_nonnegative(i64"));
    assert!(llvm.contains("icmp slt i64"));
    assert!(llvm.contains("ret i64"));
    let pinned = format!(
        "target triple = \"{}\"\ntarget datalayout = \"{}\"\n{}",
        mumei_emit_llvm::translation_validation::TARGET,
        mumei_emit_llvm::translation_validation::DATA_LAYOUT,
        llvm
    );
    let validate = |source: &str, ir: &str| {
        mumei_emit_llvm::translation_validation::validate_pure_i64(source, ir, Default::default())
            .unwrap()
            .outcome
    };
    assert_eq!(
        validate(SOURCE, &pinned),
        mumei_emit_llvm::translation_validation::Outcome::EquivalentWithinProfile
    );
    assert_eq!(
        validate(SOURCE, &pinned.replace("icmp slt", "icmp sgt")),
        mumei_emit_llvm::translation_validation::Outcome::Different
    );
    assert!(contract.requested_target_triple.is_none());
    assert_eq!(
        contract.correspondence,
        "unchecked_not_translation_validated"
    );
    std::fs::write(
        dir.path().join("semantic-contract.json"),
        serde_json::to_vec_pretty(&contract).unwrap(),
    )
    .unwrap();

    let driver = dir.path().join("observe.c");
    std::fs::write(
        &driver,
        r#"
#include <stdint.h>
extern int64_t clamp_nonnegative(int64_t);
int main(void) {
    const int64_t inputs[] = { INT64_MIN, -1, 0, 1, INT64_MAX };
    for (unsigned i = 0; i < sizeof(inputs) / sizeof(inputs[0]); ++i) {
        const int64_t expected = inputs[i] < 0 ? 0 : inputs[i];
        if (clamp_nonnegative(inputs[i]) != expected) return 1;
    }
    return 0;
}
"#,
    )
    .unwrap();
    let llvm_path = dir.path().join("slice_clamp_nonnegative.ll");
    succeeds(native_observation(&llvm_path, &driver));
    let mutated_ir = dir.path().join("wrong_comparison.ll");
    std::fs::write(&mutated_ir, llvm.replace("icmp slt i64", "icmp sgt i64")).unwrap();
    assert_eq!(
        native_observation(&mutated_ir, &driver).status.code(),
        Some(1),
        "fixed finite observations must reject intentionally wrong LLVM lowering"
    );

    // Keep the implementation fixed: a false contract must fail real Z3 checking.
    let false_source = SOURCE.replace("(x < 0 => result == 0) && (x >= 0 => result == x)", "false");
    let negative = dir.path().join("false_contract.mm");
    std::fs::write(&negative, &false_source).unwrap();
    let false_contract =
        extract_pure_i64_contract(&false_source, "false_contract.mm", None, false).unwrap();
    assert_eq!(
        contract.implementation_sha256,
        false_contract.implementation_sha256
    );
    assert_ne!(
        contract.specification_sha256,
        false_contract.specification_sha256
    );
    rejects_postcondition(&negative);

    let defective_source = SOURCE.replace("if x < 0 { 0 } else { x }", "x");
    let defective = dir.path().join("defective.mm");
    std::fs::write(&defective, &defective_source).unwrap();
    let defective_contract =
        extract_pure_i64_contract(&defective_source, "defective.mm", None, false).unwrap();
    assert_eq!(
        contract.specification_sha256,
        defective_contract.specification_sha256
    );
    assert_ne!(
        contract.implementation_sha256,
        defective_contract.implementation_sha256
    );
    rejects_postcondition(&defective);
}
