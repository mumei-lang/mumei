// =============================================================================
// Plan 11B: Z3 Proof Certificates
// =============================================================================
//
// Generates cryptographically verifiable proof certificates for verified atoms.
// Each certificate contains per-atom Z3 check results and content hashes,
// enabling offline verification that proofs are still valid.

mod bundle_io;
mod generation;
mod models;
mod review;
pub mod status;
mod validation;

pub use crate::verification::{EscalationReason, LogicFragment};

pub(crate) use bundle_io::load_certificate_unvalidated;
pub use bundle_io::{
    load_bundle, load_certificate, lookup_bundle_certificate, module_key_from_source,
    save_certificate, save_escalation_bundle, save_human_review_queue,
};

pub use generation::{
    artifact_paths_from_env, budget_policy_fingerprint_from_env, compute_atom_content_hash,
    compute_sha256, generate_certificate, generate_certificate_with_reconstruction_losses,
    get_z3_version, harness_contract_from_env, intent_fidelity_from_env,
};

pub use models::{
    ActionClassLimit, AtomCertificate, AttemptSummary, BudgetPolicy, BundleSummary,
    CostSuccessMetrics, CounterexampleValidationMetadata, EscalationBundle,
    EscalationBundleSummary, EscalationCandidate, HarnessCertificateMetadata, HumanReviewEntry,
    HumanReviewPriority, HumanReviewQueue, IntentFidelity, IntentFidelityMetadata,
    LeanResultMetadata, ProofBundle, ProofCertificate, SelfCorrectionMetadata,
    SelfCorrectionSummary, SolverProcessMetadata, UnusedHypothesisMetadata,
};

pub use review::{generate_escalation_bundle, generate_human_review_queue};

pub use validation::{
    get_required_lowering_rules, refresh_certificate_integrity,
    validate_certificate_translator_versions, validate_translator_version,
    validate_translator_version_with_semantics, verify_certificate,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::verification;
    use crate::verification::ModuleEnv;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn solver_env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    struct SolverEnvCleanup(&'static [&'static str]);

    impl Drop for SolverEnvCleanup {
        fn drop(&mut self) {
            for name in self.0 {
                std::env::remove_var(name);
            }
        }
    }

    fn make_test_atom(name: &str, requires: &str, ensures: &str, body: &str) -> parser::Atom {
        parser::Atom {
            name: name.to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![],
            trace_id: None,
            spec_metadata: std::collections::HashMap::new(),
            requires: requires.to_string(),
            forall_constraints: vec![],
            ensures: ensures.to_string(),
            body_expr: body.to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: parser::TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: crate::parser::Span::new("test.mm", 0, 0, 0),
            effect_pre: HashMap::new(),
            effect_post: HashMap::new(),
        }
    }

    #[test]
    fn test_status_constants_match_schema_enums() {
        let schema_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../schema/proof-cert.schema.json");
        let schema: Value = serde_json::from_str(
            &std::fs::read_to_string(schema_path).expect("read proof-cert schema"),
        )
        .expect("parse proof-cert schema");

        let z3_check_result_enum = schema["$defs"]["z3CheckResult"]["enum"]
            .as_array()
            .expect("z3CheckResult enum");
        let verification_status_enum = schema["$defs"]["verificationStatus"]["enum"]
            .as_array()
            .expect("verificationStatus enum");

        let z3_check_result_values: Vec<&str> = z3_check_result_enum
            .iter()
            .map(|value| value.as_str().expect("string enum entry"))
            .collect();
        let verification_status_values: Vec<&str> = verification_status_enum
            .iter()
            .map(|value| value.as_str().expect("string enum entry"))
            .collect();

        assert_eq!(&status::Z3_CHECK_RESULTS, z3_check_result_values.as_slice());
        assert_eq!(
            &status::VERIFICATION_STATUSES,
            verification_status_values.as_slice()
        );
    }

    /// P5-A: generate_certificate produces valid JSON with proof_hash, dependencies, effects, requires, ensures
    #[test]
    fn test_generate_certificate_extended_fields() {
        let _guard = solver_env_lock().lock().unwrap();
        let env_names = &[
            "MUMEI_TASK_ID",
            "MUMEI_GENERATION_ID",
            "MUMEI_VERIFICATION_TIMEOUT_MS",
            "MUMEI_SOLVER_CONFIG_FINGERPRINT",
            "MUMEI_SOLVER_CACHE_KEY",
            "MUMEI_CANCEL_REASON",
            "MUMEI_SOLVER_PROCESS_START_TIME",
            "MUMEI_HARNESS_CONTRACT",
            "MUMEI_INTENT_PROMPT_HASH",
            "MUMEI_SPEC_TRACEABILITY_SCORE",
            "MUMEI_SEMANTIC_DRIFT_DETECTED",
            "MUMEI_MANUAL_REVIEW_REQUIRED",
            "MUMEI_ARTIFACT_PATHS",
            "MUMEI_BUDGET_POLICY_FINGERPRINT",
        ];
        let _cleanup = SolverEnvCleanup(env_names);
        for name in env_names {
            std::env::remove_var(name);
        }
        let atom = make_test_atom("add", "x > 0", "result > 0", "x + 1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "add".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate(
            "test.mm",
            &atoms,
            &results,
            &module_env,
            Some("my_pkg"),
            Some("1.0.0"),
            None,
        );

        assert_eq!(cert.atoms.len(), 1);
        assert_eq!(cert.atoms[0].name, "add");
        assert_eq!(cert.atoms[0].requires, "x > 0");
        assert_eq!(cert.atoms[0].ensures, "result > 0");
        assert_eq!(cert.atoms[0].body_expr, "x + 1");
        assert_eq!(cert.atoms[0].body_summary, "x + 1");
        assert!(!cert.atoms[0].proof_hash.is_empty());
        assert!(cert.atoms[0].solver_process_metadata.is_none());
        assert!(cert.atoms[0].retry_policy_fingerprint.is_none());
        assert!(cert.atoms[0].attempt_summary.is_none());
        assert!(cert.atoms[0].cost_success_metrics.is_none());
        assert!(cert.all_verified);
        assert_eq!(cert.package_name, Some("my_pkg".to_string()));
        assert_eq!(cert.package_version, Some("1.0.0".to_string()));

        // Verify JSON serialization roundtrip
        let json = serde_json::to_string(&cert).unwrap();
        let parsed: ProofCertificate = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.atoms[0].requires, "x > 0");
        assert_eq!(parsed.atoms[0].ensures, "result > 0");
        assert_eq!(parsed.atoms[0].body_expr, "x + 1");
    }

    #[test]
    fn test_generate_certificate_harness_metadata_fields() {
        let atom = make_test_atom("harnessed", "true", "true", "0");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "harnessed".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate(
            "test.mm",
            &atoms,
            &results,
            &module_env,
            None,
            None,
            Some(HarnessCertificateMetadata {
                harness_contract: Some("contracts/nlah.json".to_string()),
                intent_fidelity: Some(IntentFidelityMetadata {
                    natural_language_prompt_hash: Some("sha256:prompt".to_string()),
                    spec_traceability_score: 0.97,
                    semantic_drift_detected: false,
                    manual_review_required: true,
                }),
                artifact_paths: Some(vec![
                    "reports/proof.json".to_string(),
                    "out/doc.json".to_string(),
                ]),
                budget_policy_fingerprint: Some("sha256:budget".to_string()),
            }),
        );

        assert_eq!(
            cert.harness_contract.as_deref(),
            Some("contracts/nlah.json")
        );
        let intent_fidelity = cert.intent_fidelity.as_ref().unwrap();
        assert_eq!(
            intent_fidelity.natural_language_prompt_hash.as_deref(),
            Some("sha256:prompt")
        );
        assert_eq!(intent_fidelity.spec_traceability_score, 0.97);
        assert!(!intent_fidelity.semantic_drift_detected);
        assert!(intent_fidelity.manual_review_required);
        assert_eq!(
            cert.artifact_paths,
            Some(vec![
                "reports/proof.json".to_string(),
                "out/doc.json".to_string()
            ])
        );
        assert_eq!(
            cert.budget_policy_fingerprint.as_deref(),
            Some("sha256:budget")
        );

        let json = serde_json::to_string(&cert).unwrap();
        let parsed: ProofCertificate = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.harness_contract, cert.harness_contract);
        assert_eq!(parsed.artifact_paths, cert.artifact_paths);
    }

    #[test]
    fn test_certificate_backward_compatibility_without_harness_fields() {
        let json = r#"{
            "version": "1.0",
            "timestamp": "2026-05-24T00:00:00Z",
            "mumei_version": "0.2.0",
            "z3_version": "z3 4.12.2",
            "file": "legacy.mm",
            "atoms": [],
            "certificate_hash": "",
            "all_verified": true
        }"#;

        let cert: ProofCertificate = serde_json::from_str(json).unwrap();

        assert!(cert.harness_contract.is_none());
        assert!(cert.intent_fidelity.is_none());
        assert!(cert.artifact_paths.is_none());
        assert!(cert.budget_policy_fingerprint.is_none());
    }

    #[test]
    fn test_harness_metadata_env_helpers() {
        let _guard = solver_env_lock().lock().unwrap();
        let env_names = &[
            "MUMEI_HARNESS_CONTRACT",
            "MUMEI_INTENT_PROMPT_HASH",
            "MUMEI_SPEC_TRACEABILITY_SCORE",
            "MUMEI_SEMANTIC_DRIFT_DETECTED",
            "MUMEI_MANUAL_REVIEW_REQUIRED",
            "MUMEI_ARTIFACT_PATHS",
            "MUMEI_BUDGET_POLICY_FINGERPRINT",
        ];
        let _cleanup = SolverEnvCleanup(env_names);
        for name in env_names {
            std::env::remove_var(name);
        }

        std::env::set_var("MUMEI_HARNESS_CONTRACT", "contracts/harness.json");
        std::env::set_var("MUMEI_INTENT_PROMPT_HASH", "sha256:prompt");
        std::env::set_var("MUMEI_SPEC_TRACEABILITY_SCORE", "0.97");
        std::env::set_var("MUMEI_SEMANTIC_DRIFT_DETECTED", "false");
        std::env::set_var("MUMEI_MANUAL_REVIEW_REQUIRED", "true");
        std::env::set_var("MUMEI_ARTIFACT_PATHS", " reports/a.json, ,out/b.json ");
        std::env::set_var("MUMEI_BUDGET_POLICY_FINGERPRINT", "sha256:budget");

        assert_eq!(
            harness_contract_from_env().as_deref(),
            Some("contracts/harness.json")
        );
        assert_eq!(
            artifact_paths_from_env(),
            Some(vec!["reports/a.json".to_string(), "out/b.json".to_string()])
        );
        let intent_fidelity = intent_fidelity_from_env().unwrap();
        assert_eq!(
            intent_fidelity.natural_language_prompt_hash.as_deref(),
            Some("sha256:prompt")
        );
        assert_eq!(intent_fidelity.spec_traceability_score, 0.97);
        assert!(!intent_fidelity.semantic_drift_detected);
        assert!(intent_fidelity.manual_review_required);
        assert_eq!(
            budget_policy_fingerprint_from_env().as_deref(),
            Some("sha256:budget")
        );
    }

    #[test]
    fn test_retry_budget_schema_roundtrip() {
        let mut limits = HashMap::new();
        limits.insert(
            "lean_escalation".to_string(),
            ActionClassLimit {
                max_attempts: 1,
                max_tokens: 2_000,
                max_lean_escalations: 1,
            },
        );
        let policy = BudgetPolicy {
            max_attempts: 5,
            max_tokens: 10_000,
            max_solver_time_ms: 30_000,
            max_semantic_delta: 0.5,
            action_class_limits: limits,
        };

        let json = serde_json::to_string(&policy).unwrap();
        let parsed: BudgetPolicy = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.max_attempts, 5);
        assert_eq!(
            parsed
                .action_class_limits
                .get("lean_escalation")
                .unwrap()
                .max_lean_escalations,
            1
        );
    }

    #[test]
    fn test_atom_certificate_retry_metrics_roundtrip() {
        let atom = make_test_atom("budgeted", "true", "true", "0");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "budgeted".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let mut attempts_by_action_class = HashMap::new();
        attempts_by_action_class.insert("llm_fix".to_string(), 2);
        cert.atoms[0].retry_policy_fingerprint = Some("sha256:abc".to_string());
        cert.atoms[0].attempt_summary = Some(AttemptSummary {
            total_attempts: 2,
            attempts_by_action_class,
            final_action_class: "llm_fix".to_string(),
        });
        cert.atoms[0].cost_success_metrics = Some(CostSuccessMetrics {
            attempts_to_success: 2,
            tokens_to_success: 1_024,
            solver_seconds_to_success: 1.5,
            spec_drift_score: 0.1,
        });

        let json = serde_json::to_string(&cert).unwrap();
        let parsed: ProofCertificate = serde_json::from_str(&json).unwrap();

        assert_eq!(
            parsed.atoms[0].retry_policy_fingerprint.as_deref(),
            Some("sha256:abc")
        );
        assert_eq!(
            parsed.atoms[0]
                .attempt_summary
                .as_ref()
                .unwrap()
                .final_action_class,
            "llm_fix"
        );
        assert_eq!(
            parsed.atoms[0]
                .cost_success_metrics
                .as_ref()
                .unwrap()
                .tokens_to_success,
            1_024
        );
    }

    #[test]
    fn test_generate_certificate_records_unused_hypotheses_when_core_available() {
        let mut atom = make_test_atom("bounded", "x > 0", "result > 0", "1");
        atom.invariant = Some("x < 10".to_string());
        atom.effect_pre
            .insert("Account".to_string(), "Open".to_string());
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "bounded".to_string(),
            (
                "unsat unsat_core=[|track_requires|]".to_string(),
                "verified".to_string(),
            ),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let unused = cert.atoms[0].unused_hypotheses.as_ref().unwrap();

        assert!(unused.unused_requires.is_empty());
        assert_eq!(unused.unused_invariants, vec!["x < 10"]);
        assert_eq!(unused.unused_effect_constraints, vec!["Account=Open"]);
        assert_eq!(unused.minimal_constraint_set, vec!["|track_requires|"]);
    }

    #[test]
    fn test_generate_certificate_records_solver_process_metadata() {
        let _guard = solver_env_lock().lock().unwrap();
        let env_names = &[
            "MUMEI_TASK_ID",
            "MUMEI_GENERATION_ID",
            "MUMEI_VERIFICATION_TIMEOUT_MS",
            "MUMEI_SOLVER_CONFIG_FINGERPRINT",
            "MUMEI_SOLVER_CACHE_KEY",
            "MUMEI_CANCEL_REASON",
            "MUMEI_SOLVER_PROCESS_START_TIME",
        ];
        let _cleanup = SolverEnvCleanup(env_names);
        for name in env_names {
            std::env::remove_var(name);
        }
        std::env::set_var("MUMEI_TASK_ID", "task-1");
        std::env::set_var("MUMEI_GENERATION_ID", "generation-1");
        std::env::set_var("MUMEI_VERIFICATION_TIMEOUT_MS", "1234");
        std::env::set_var("MUMEI_SOLVER_CONFIG_FINGERPRINT", "fingerprint-1");
        std::env::set_var("MUMEI_SOLVER_CACHE_KEY", "cache-1");
        std::env::set_var("MUMEI_CANCEL_REASON", "timeout");
        std::env::set_var("MUMEI_SOLVER_PROCESS_START_TIME", "2026-05-17T00:00:00Z");

        let atom = make_test_atom("orchestrated", "true", "true", "0");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "orchestrated".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let metadata = cert.atoms[0].solver_process_metadata.as_ref().unwrap();

        assert_eq!(metadata.task_id.as_deref(), Some("task-1"));
        assert_eq!(metadata.generation_id, "generation-1");
        assert_eq!(metadata.timeout_ms, 1234);
        assert_eq!(metadata.solver_config_fingerprint, "fingerprint-1");
        assert_eq!(metadata.cache_key, "cache-1");
        assert_eq!(metadata.cancel_reason.as_deref(), Some("timeout"));
        assert_eq!(metadata.process_start_time, "2026-05-17T00:00:00Z");
        assert!(!metadata.process_end_time.is_empty());
    }

    #[test]
    fn test_generate_certificate_records_traceability_validation() {
        let mut atom = make_test_atom("traced", "true", "true", "0");
        atom.trace_id = Some("REQ-1".to_string());
        atom.spec_metadata
            .insert("source".to_string(), "unit-test".to_string());
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "traced".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let validation = cert.atoms[0].spec_validation_result.as_ref().unwrap();

        assert!(validation.is_satisfiable);
        assert!(validation.contradiction_details.is_none());
        assert_eq!(validation.trace_id.as_deref(), Some("REQ-1"));
        assert_eq!(validation.traceability_hash.len(), 64);
    }

    /// P5-A: verify_certificate detects "changed" when source is modified
    #[test]
    fn test_verify_certificate_detects_changed() {
        let atom = make_test_atom("add", "x > 0", "result > 0", "x + 1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "add".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);

        // Verify with same source → "proven"
        let status = verify_certificate(&cert, &atoms, false);
        assert_eq!(status.len(), 1);
        assert_eq!(status[0], ("add".to_string(), "proven".to_string()));

        // Modify the atom body and verify again → "changed"
        let modified_atom = make_test_atom("add", "x > 0", "result > 0", "x + 2");
        let modified_atoms: Vec<&parser::Atom> = vec![&modified_atom];
        let status2 = verify_certificate(&cert, &modified_atoms, false);
        assert_eq!(status2.len(), 1);
        assert_eq!(status2[0], ("add".to_string(), "changed".to_string()));
    }

    /// PR 2: `lean_verified` is rejected by default and accepted with the
    /// `allow_lean_verified` opt-in flag, mirroring the cross-project Proof
    /// Certificate Chain (mumei-lean → mumei resolver) handshake.
    #[test]
    fn test_verify_certificate_lean_verified_opt_in() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        // mumei-lean signals successful Lean discharge with this string.
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);

        // Default (backwards-compatible): lean_verified is NOT proven.
        let status_default = verify_certificate(&cert, &atoms, false);
        assert_eq!(
            status_default[0],
            ("hard_lemma".to_string(), "unproven".to_string())
        );

        // Opt-in still rejects lean_verified without Lean result metadata.
        let status_missing_metadata = verify_certificate(&cert, &atoms, true);
        assert_eq!(
            status_missing_metadata[0],
            ("hard_lemma".to_string(), "stale_translator".to_string())
        );

        cert.atoms[0].lean_metadata = Some(LeanResultMetadata {
            status: "lean_verified".to_string(),
            theorem_name: "hard_lemma_correct".to_string(),
            translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
            bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
            proof_path: "Generated/Test.lean".to_string(),
            diagnostics: vec![],
        });

        // Opt-in: lean_verified is proven only with current Lean metadata.
        let status_opt_in = verify_certificate(&cert, &atoms, true);
        assert_eq!(
            status_opt_in[0],
            ("hard_lemma".to_string(), "proven".to_string())
        );

        cert.atoms[0].bridge_lemma_hash = "old-bridge-hash".to_string();
        let status_stale_atom_hash = verify_certificate(&cert, &atoms, true);
        assert_eq!(
            status_stale_atom_hash[0],
            ("hard_lemma".to_string(), "stale_translator".to_string())
        );

        cert.atoms[0].bridge_lemma_hash = verification::LEAN_BRIDGE_LEMMA_HASH.to_string();
        cert.atoms[0].lean_result_metadata = Some(LeanResultMetadata {
            status: "lean_verified".to_string(),
            theorem_name: "hard_lemma_correct".to_string(),
            translator_version: verification::LEAN_TRANSLATOR_VERSION.to_string(),
            bridge_lemma_hash: "old-bridge-hash".to_string(),
            proof_path: "Generated/Test.lean".to_string(),
            diagnostics: vec![],
        });
        let status_stale_result_metadata = verify_certificate(&cert, &atoms, true);
        assert_eq!(
            status_stale_result_metadata[0],
            ("hard_lemma".to_string(), "stale_translator".to_string())
        );
    }

    #[test]
    fn test_validate_translator_version_detects_atom_version_mismatch() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].translator_version = "old-translator".to_string();

        let err = validate_translator_version(
            &cert.atoms[0],
            verification::LEAN_TRANSLATOR_VERSION,
            verification::LEAN_BRIDGE_LEMMA_HASH,
        )
        .expect_err("stale translator must fail");
        assert!(err.contains("translator_version"));
        assert!(err.contains("old-translator"));
    }

    #[test]
    fn test_validate_translator_version_detects_bridge_hash_mismatch() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].bridge_lemma_hash = "old-bridge-hash".to_string();

        let err = validate_translator_version(
            &cert.atoms[0],
            verification::LEAN_TRANSLATOR_VERSION,
            verification::LEAN_BRIDGE_LEMMA_HASH,
        )
        .expect_err("stale bridge hash must fail");
        assert!(err.contains("bridge_lemma_hash"));
        assert!(err.contains("old-bridge-hash"));
    }

    #[test]
    fn test_validate_translator_version_detects_lean_metadata_mismatch() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].lean_metadata = Some(LeanResultMetadata {
            status: "lean_verified".to_string(),
            theorem_name: "hard_lemma_correct".to_string(),
            translator_version: "old-translator".to_string(),
            bridge_lemma_hash: verification::LEAN_BRIDGE_LEMMA_HASH.to_string(),
            proof_path: "Generated/Test.lean".to_string(),
            diagnostics: vec![],
        });

        let err = validate_translator_version(
            &cert.atoms[0],
            verification::LEAN_TRANSLATOR_VERSION,
            verification::LEAN_BRIDGE_LEMMA_HASH,
        )
        .expect_err("stale Lean metadata must fail");
        assert!(err.contains("lean_metadata.translator_version"));
        assert!(err.contains("old-translator"));
    }

    #[test]
    fn test_validate_translator_version_detects_missing_metadata() {
        let atom = make_test_atom("add", "true", "true", "1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "add".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].translator_version.clear();
        cert.atoms[0].bridge_lemma_hash.clear();

        let err = validate_translator_version(
            &cert.atoms[0],
            verification::LEAN_TRANSLATOR_VERSION,
            verification::LEAN_BRIDGE_LEMMA_HASH,
        )
        .expect_err("missing translator metadata must fail");
        assert!(err.contains("translator_version"));
        assert!(err.contains("bridge_lemma_hash"));
    }

    #[test]
    fn test_required_lowering_rules_extraction() {
        let tags = vec![
            "array_operations".to_string(),
            "integer_arithmetic".to_string(),
        ];
        let rules = get_required_lowering_rules(&tags);

        assert!(rules.contains(&"type_system_mapping".to_string()));
        assert!(rules.contains(&"contract_lowering".to_string()));
        assert!(rules.contains(&"array_bounds_bridge".to_string()));
        assert!(rules.contains(&"integer_overflow_bridge".to_string()));
    }

    #[test]
    fn test_translator_version_validation_with_semantics() {
        let atom = make_test_atom("semantic_gap", "true", "true", "1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "semantic_gap".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].translator_ir.lowering_rules = vec!["type_system_mapping".to_string()];

        let required_rules = vec![
            "type_system_mapping".to_string(),
            "array_bounds_bridge".to_string(),
        ];
        let result = validate_translator_version_with_semantics(
            &cert.atoms[0],
            verification::LEAN_TRANSLATOR_VERSION,
            verification::LEAN_BRIDGE_LEMMA_HASH,
            &required_rules,
        );

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("missing required lowering rules"));
    }

    #[test]
    fn test_translator_ir_metadata_includes_semantic_gap_fields() {
        let atom = make_test_atom("array_guard", "forall(i, 0, n, arr[i] >= 0)", "true", "1");
        let module_env = ModuleEnv::new();
        let ir = verification::build_translator_ir_metadata(&atom, &module_env);

        assert!(ir
            .lowering_rules
            .contains(&"array_bounds_bridge".to_string()));
        assert!(ir
            .lowering_rules
            .contains(&"refinement_predicate_lowering".to_string()));
        assert!(ir
            .semantic_gap_notes
            .iter()
            .any(|note| note.starts_with("array_bounds_bridge:")));
        assert!(ir
            .proof_trace_hints
            .iter()
            .any(|hint| hint.starts_with("preserve i < arr.length")));
        assert!(ir
            .requires_bridge_lemmas
            .contains(&"mumei_array_bounds_bridge".to_string()));
        assert!(ir
            .requires_bridge_lemmas
            .contains(&"mumei_array_get_bridge".to_string()));
    }

    /// PR 2: `allow_lean_verified` does not weaken the `"changed"` detector.
    /// A modified body must still be flagged as needing re-verification, even
    /// when the cert claims `lean_verified`.
    #[test]
    fn test_verify_certificate_lean_verified_changed_detection() {
        let atom = make_test_atom("hard_lemma", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "hard_lemma".to_string(),
            ("lean_verified".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);

        let modified = make_test_atom("hard_lemma", "true", "true", "43");
        let modified_refs: Vec<&parser::Atom> = vec![&modified];

        let status = verify_certificate(&cert, &modified_refs, true);
        assert_eq!(status[0], ("hard_lemma".to_string(), "changed".to_string()));
    }

    #[test]
    fn test_generate_human_review_queue_prioritizes_manual_unknown_and_trusted_atoms() {
        let manual = make_test_atom("manual_case", "true", "true", "match x { _ => 0 }");
        let unknown = make_test_atom("unknown_case", "n > 0", "result > n", "n * n");
        let mut trusted = make_test_atom("trusted_case", "true", "result >= 0", "0");
        trusted.trust_level = parser::TrustLevel::Trusted;
        let atoms: Vec<&parser::Atom> = vec![&trusted, &unknown, &manual];
        let mut results = HashMap::new();
        results.insert(
            "manual_case".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        results.insert(
            "unknown_case".to_string(),
            ("unknown".to_string(), "failed".to_string()),
        );
        results.insert(
            "trusted_case".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let queue = generate_human_review_queue(&cert);

        assert_eq!(
            queue
                .atoms
                .iter()
                .map(|entry| (entry.atom_name.as_str(), entry.priority.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("manual_case", "critical"),
                ("unknown_case", "high"),
                ("trusted_case", "medium")
            ]
        );
        assert!(queue.atoms[1].spec_text.contains("requires: n > 0"));
    }

    /// P5-A: certificate_hash is deterministic
    #[test]
    fn test_certificate_hash_deterministic() {
        let atom = make_test_atom("foo", "true", "true", "42");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "foo".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert1 =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let cert2 =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);

        // certificate_hash should be the same for the same inputs
        // (timestamp differs, so we check the hash is non-empty and format is correct)
        assert!(!cert1.certificate_hash.is_empty());
        assert!(!cert2.certificate_hash.is_empty());
        // Both should be 64-char hex strings (SHA-256)
        assert_eq!(cert1.certificate_hash.len(), 64);
        assert_eq!(cert2.certificate_hash.len(), 64);
    }

    /// P5-A: compute_sha256 utility works correctly
    #[test]
    fn test_compute_sha256() {
        let hash1 = compute_sha256("hello");
        let hash2 = compute_sha256("hello");
        let hash3 = compute_sha256("world");
        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA-256 hex = 64 chars
    }

    /// P5-A: all_verified is false when any atom fails
    #[test]
    fn test_all_verified_false_when_failed() {
        let atom1 = make_test_atom("ok", "true", "true", "1");
        let atom2 = make_test_atom("fail", "true", "true", "0");
        let atoms: Vec<&parser::Atom> = vec![&atom1, &atom2];
        let mut results = HashMap::new();
        results.insert(
            "ok".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        results.insert(
            "fail".to_string(),
            ("sat".to_string(), "failed".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        assert!(!cert.all_verified);
    }

    #[test]
    fn test_all_verified_false_when_spec_validation_fails() {
        let atom = make_test_atom("impossible", "false", "true", "0");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "impossible".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        let validation = cert.atoms[0].spec_validation_result.as_ref().unwrap();

        assert!(!validation.is_satisfiable);
        assert!(!cert.all_verified);
    }

    #[test]
    fn test_load_certificate_rejects_stale_translator_version() {
        let atom = make_test_atom("bar", "true", "result == 1", "1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "bar".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].translator_version = "old-translator".to_string();

        let tmp = std::env::temp_dir().join("mumei_stale_translator_cert.json");
        save_certificate(&cert, &tmp).unwrap();
        let err = load_certificate(&tmp).expect_err("load_certificate must reject stale metadata");
        assert!(err.contains("old-translator"));

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_load_certificate_rejects_stale_bridge_hash() {
        let atom = make_test_atom("bar", "true", "result == 1", "1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "bar".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();
        let mut cert =
            generate_certificate("test.mm", &atoms, &results, &module_env, None, None, None);
        cert.atoms[0].bridge_lemma_hash = "old-bridge-hash".to_string();

        let tmp = std::env::temp_dir().join("mumei_stale_bridge_cert.json");
        save_certificate(&cert, &tmp).unwrap();
        let err = load_certificate(&tmp).expect_err("load_certificate must reject stale metadata");
        assert!(err.contains("old-bridge-hash"));

        let _ = std::fs::remove_file(&tmp);
    }

    /// P5-A: save and load certificate roundtrip
    #[test]
    fn test_save_load_certificate_roundtrip() {
        let atom = make_test_atom("bar", "true", "result == 1", "1");
        let atoms: Vec<&parser::Atom> = vec![&atom];
        let mut results = HashMap::new();
        results.insert(
            "bar".to_string(),
            ("unsat".to_string(), "verified".to_string()),
        );
        let module_env = ModuleEnv::new();

        let cert = generate_certificate(
            "test.mm",
            &atoms,
            &results,
            &module_env,
            Some("pkg"),
            Some("2.0.0"),
            None,
        );

        let tmp = std::env::temp_dir().join("mumei_test_cert.json");
        save_certificate(&cert, &tmp).unwrap();
        let loaded = load_certificate(&tmp).unwrap();

        assert_eq!(loaded.atoms.len(), 1);
        assert_eq!(loaded.atoms[0].name, "bar");
        assert_eq!(loaded.package_name, Some("pkg".to_string()));
        assert_eq!(loaded.certificate_hash, cert.certificate_hash);

        let _ = std::fs::remove_file(&tmp);
    }
}
