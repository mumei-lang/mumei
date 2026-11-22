# Canonical MCP Tool Contract

This is the canonical tool contract for both the `mumei-forge` and
`mumei-agent` MCP servers.  The `mumei-agent` `docs/MCP_SERVER.md` table is
subordinate to this document.  Tool names, argument annotations, defaults,
and documented return keys below are extracted from the decorated functions
in each server; an empty return-key cell means the implementation does not
promise a fixed object-key set.

The contract vocabulary is not aliased.  The harness keys are
`harness_contract`, `intent_fidelity`, `artifact_paths`,
`budget_policy_fingerprint`, and `lean_verified`.  The eight no-`.mm` keys
are `spec_health_issues`, `verification_violations`, `verification_status`,
`cross_validation_gaps`, `next_steps`, `migration_hints`, `healed_files`, and
`heal_errors`.  `scan_and_fix` == `audit --auto-migrate --auto-heal`.

## `mumei-forge`

Source: `mcp_server.py` in the mumei repository.

| Tool | Arguments | Documented return keys |
| --- | --- | --- |
| `get_spec_guideline` |  |  |
| `get_spec_guidelines` |  |  |
| `forge_blade` | `source_code: str, output_name: str = "katana", trace_id: Optional[str] = None, spec_metadata: Optional[Dict[str, str]] = None` |  |
| `validate_logic` | `source_code: str, trace_id: Optional[str] = None, spec_metadata: Optional[Dict[str, str]] = None` |  |
| `get_structured_feedback` | `source_code: str` |  |
| `analyze_contract_conflicts` | `source_code: str` |  |
| `propose_interface_refactoring` | `source_code: str, retry_history: dict \| None = None` |  |
| `verify_with_orchestration` | `source_code: str, timeout_ms: int = 30000, enable_cache: bool = True, task_id: str \| None = None` |  |
| `execute_mm` | `source_code: str, output_name: str = "katana", command: str = "build"` |  |
| `get_inferred_effects` | `source_code: str` |  |
| `get_allowed_effects` | `project_dir: str = "."` |  |
| `set_allowed_effects` | `allowed: "list[str] \| None" = None, denied: "list[str] \| None" = None, project_dir: str = "."` |  |
| `list_std_catalog` |  |  |
| `visualize_std_graph` | `format: str = "mermaid"` |  |
| `visualize_proof_graph` | `source_code: str, format: str = "json"` |  |
| `analyze_std_gaps` |  | `dependency_graph`, `trusted_atoms`, `todo_comments`, `usage_frequency`, `proposals` |
| `measure_std_health` |  | `total_files`, `verified_files`, `failed_files`, `total_atoms`, `verified_atoms`, `trusted_atoms`, `health_score`, `todo_count`, `details` |
| `get_proof_certificate` | `module_path: str` | `certificate`, `error` |
| `generate_doc` | `source_code: str, format: str = "json"` |  |

## `mumei-agent`

Source: `/home/ubuntu/repos/mumei-agent/agent/mcp_server.py`.
Agent table is exact against mumei-agent origin/develop at commit `ddda440fdff7ef77c3cd468dfd00af95e12f9680`.

| Tool | Arguments | Documented return keys |
| --- | --- | --- |
| `get_spec_guide_summary` |  |  |
| `get_spec_guidelines` |  |  |
| `forge_task` | `task_json: str, mumei_repo: str, dry_run: bool = True, ctx: Context \| None = None` | `task_id`, `status`, `target_file`, `error`, `code_length` |
| `heal_file` | `source_code: str = '', error_report: str = '', code_file: str = '', ctx: Context \| None = None` | `healed_code`, `attempts`, `success`, `error` |
| `self_correct` | `code_file: str, max_iterations: int = 10, ctx: Context \| None = None` |  |
| `run_nlae_pipeline` | `spec: str, mumei_lean_repo: str = '', work_dir: str = '', no_build: bool = False, multi_agent: bool = False` |  |
| `measure_std_health` | `mumei_repo: str` | `total_files`, `verified_files`, `failed_files`, `total_atoms`, `verified_atoms`, `trusted_atoms`, `health_score`, `todo_count`, `details` |
| `cross_validate` | `spec_file: str, impl_file: str, language: str = ''` | `spec_stronger_than_impl`, `impl_stronger_than_spec`, `uncovered_atoms`, `coverage_ratio`, `details` |
| `propose_forge_tasks` | `mumei_repo: str, max_proposals: int = 3` | `proposals`, `specs` |
| `list_forge_log` | `log_path: str = 'forge_log.json'` | `entries`, `count` |
| `get_review_queue` | `mumei_repo: str` |  |
| `approve_review` | `atom_name: str, reviewer: str, notes: str` |  |
| `escalate_to_lean` | `atom_name: str` |  |
| `reject_review` | `atom_name: str, reviewer: str, notes: str` |  |
| `get_agent_status` | `ctx: Context \| None = None` |  |
| `send_latent_message` | `message: str, context: str = '{}', verify: bool = True` |  |
| `send_latent_message_batch` | `messages: str, verify: bool = False` |  |
| `async_send_latent_message` | `message: str, context: str = '{}', verify: bool = True` |  |
| `extract_spec` | `natural_language: str, domain_hint: str = '', generate: bool = False, mumei_repo: str = '', check_contradiction_only: bool = False, ctx: Context \| None = None` | `spec`, `code`, `verified` |
| `check_spec_contradiction` | `natural_language: str, domain_hint: str = '', ctx: Context \| None = None` |  |
| `check_cross_spec_consistency` | `spec_files: str` |  |
| `check_spec_health` | `source_code: str, mumei_repo: str = ''` | `contradictions`, `over_constrained`, `vacuous`, `health_score` |
| `validate_nl_spec` | `spec_text: str, use_llm: bool = True, run_mumei: bool = True, domain_hint: str = '', ctx: Context \| None = None` |  |
| `validate_nl_spec_multi` | `spec_texts_json: str, domain_hint: str = '', use_llm: bool = True, ctx: Context \| None = None` |  |
| `validate_code` | `code: str, language: str, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `validate_foreign_code` | `code: str, language: str, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `validate_spec_to_code` | `spec: str, code_path: str, language: str \| None = None, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `validate_code_to_spec` | `code_path: str, spec_path: str, language: str \| None = None, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `verify_conformance` | `spec: str, code_path: str, language: str \| None = None, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `verify_code_spec_traceability` | `code_file: str, spec_text: str, language: str \| None = None, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `verify_foreign_code` | `source_code: str, language: str, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `audit_code` | `source_code: str, language: str, domain_hint: str = '', ctx: Context \| None = None` |  |
| `go_summaries` | `source_files: str, go_types: str, target_arch: str = 'amd64', imports_json: str = '{}', timeout_s: int = 30` | `manifest` |
| `go_summaries_attempt` | `source_files: str, go_types: str, attempt_timeout_ms: int, target_arch: str = 'amd64', imports_json: str = '{}', timeout_s: int = 30, memory_bound_mb: int = 0` | `report` |
| `go_summaries_reuse_attempt` | `source_files: str, go_types: str, manifest_path: str, attempt_timeout_ms: int, target_arch: str = 'amd64', imports_json: str = '{}', timeout_s: int = 30, memory_bound_mb: int = 0` | `report` |
| `go_summaries_check` | `source_files: str, go_types: str, manifest_path: str, target_arch: str = 'amd64', imports_json: str = '{}', timeout_s: int = 30` | `check` |
| `go_summaries_reuse` | `source_files: str, go_types: str, manifest_path: str, target_arch: str = 'amd64', imports_json: str = '{}', timeout_s: int = 30` | `manifest` |
| `go_summaries_accept` | `source_files: str, go_types: str, manifest_path: str, binding: str, target_arch: str = 'amd64', imports_json: str = '{}', timeout_s: int = 30` | `accept` |
| `evidence_identity_check` | `evidence_path: str, source_path: str` | `identity` |
| `evidence_producer_check` | `evidence_path: str, tool: str, adapter_id: str, tool_version: str = '', adapter_version: str = ''` | `identity` |
| `evidence_compare` | `evidence_a_path: str, evidence_b_path: str` | `comparison` |
| `evidence_check_accept` | `sequence_json: str, check_json: str` | `acceptance` |
| `evidence_acceptance_eval` | `descriptor_path: str, expected_fingerprint: str \| None = None` | `evaluation` |
| `freshness_eval` | `record_path: str, reference_time: str, max_age_seconds: StrictInt` | `evaluation` |
| `binding_eval` | `record_path: str, evidence_path: str` | `binding` |
| `recovery_eval` | `record_path: str, evidence_paths: list[str]` | `recovery` |
| `signature_eval` | `envelope_path: str, expected_subject: str \| None = None, expected_producer_key: str \| None = None, revocation_list_path: str \| None = None, key_registry_path: str \| None = None, registry_signature_path: str \| None = None, revocation_signature_path: str \| None = None, reference_time: str \| None = None, max_age_seconds: StrictInt \| None = None` | `evaluation` |
| `signature_verify` | `envelope_path: str, producer_key: str, expected_subject: str \| None = None, expected_producer_key: str \| None = None, revocation_list_path: str \| None = None, key_registry_path: str \| None = None, registry_signature_path: str \| None = None, revocation_signature_path: str \| None = None, reference_time: str \| None = None, max_age_seconds: StrictInt \| None = None, registry_key: str \| None = None, revocation_key: str \| None = None` | `verification` |
| `verify_attempt` | `core_path: str, source_path: str, solver_timeout_ms: int = 5000, solver_resource_limit: int = 1000000, attempt_timeout_ms: int = 30000` | `report` |
| `verify_recheck` | `core_path: str, evidence_path: str, source_path: str, solver_timeout_ms: int = 5000, solver_resource_limit: int = 1000000, attempt_timeout_ms: int = 30000` | `report` |
| `verify_accept` | `core_path: str, evidence_path: str, source_path: str, solver_timeout_ms: int = 5000, solver_resource_limit: int = 1000000, attempt_timeout_ms: int = 30000` | `report` |
| `verify_schedule` | `request_path: str, adapters_path: str, argv_map_path: str, policy_path: str, source_path: str, observations_path: str \| None = None, max_attempts: int = 8, memory_bound_mb: int \| None = None, core_path: str \| None = None, core_requirement_path: str \| None = None, core_requirement_list_path: str \| None = None` | `report` |
| `verify_plan` | `request_path: str, adapters_path: str, policy_path: str, observations_path: str \| None = None` | `report` |
| `evidence_check_adapter` | `core_path: str, manifest_path: str, attempt_timeout_ms: int = 30000` | `report` |
| `evidence_check_schedule` | `core_path: str, schedule_path: str, attempt_timeout_ms: int = 30000` | `report` |
| `evidence_check` | `core_path: str, evidence_path: str, attempt_timeout_ms: int = 30000, expected_adapter: str \| None = None` | `report` |
| `evidence_check_attempt_acceptance` | `core_path: str, acceptance_path: str, attempt_timeout_ms: int = 30000` | `report` |
| `evidence_check_list` | `core_path: str, evidence_path: str, attempt_timeout_ms: int = 30000, expected_adapter: str \| None = None` | `report` |
| `evidence_accept_list` | `core_path: str, list_path: str, requirement_path: str = '', requirement_list_path: str = '', attempt_timeout_ms: int = 30000` | `report` |
| `evidence_check_consistency` | `core_path: str, evidence_path: str, attempt_timeout_ms: int = 30000, expected_adapter: str \| None = None` | `report` |
| `native_recheck` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 30000` | `report` |
| `native_accept` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_object_accept` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_artifact_recheck` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_exit_status_recheck` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_exit_status_accept` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_stdout_recheck` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_stdout_accept` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_observations_accept` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_object_recheck` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_machine_tv_recheck` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_observations_recheck` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_stderr_recheck` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_stderr_accept` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 60000` | `report` |
| `native_check` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 30000` | `report` |
| `native_source_recheck` | `core_path: str, bundle_dir: str, source_path: str, expected_build_id: str, attempt_timeout_ms: int = 30000` | `report` |
| `native_evidence_recheck` | `core_path: str, bundle_dir: str, source_path: str, evidence_path: str, expected_build_id: str, attempt_timeout_ms: int = 30000` | `report` |
| `native_evidence_accept` | `core_path: str, bundle_dir: str, source_path: str, evidence_path: str, expected_build_id: str, attempt_timeout_ms: int = 30000` | `report` |
| `native_attempt` | `core_path: str, source_path: str, bundle_dir: str, attempt_timeout_ms: int = 60000` | `report` |
| `summaries_plan` | `profile: str, stamp: int, tool_version: str = '', histories_path: str = '', rounds: int = 1, max_parallel: int = 1` | `plan`, `observations`, `histories` |
| `ts_summaries` | `source_path: str, resolve_modules: bool = False` | `manifest` |
| `rust_summaries` | `source_path: str, resolve_modules: bool = False` | `manifest` |
| `sol_summaries` | `source_path: str, resolve_modules: bool = False` | `manifest` |
| `ts_summaries_check` | `source_path: str, manifest_path: str, resolve_modules: bool = False` | `check` |
| `rust_summaries_check` | `source_path: str, manifest_path: str, resolve_modules: bool = False` | `check` |
| `sol_summaries_check` | `source_path: str, manifest_path: str, resolve_modules: bool = False` | `check` |
| `ts_summaries_reuse` | `source_path: str, manifest_path: str, resolve_modules: bool = False` | `manifest` |
| `rust_summaries_reuse` | `source_path: str, manifest_path: str, resolve_modules: bool = False` | `manifest` |
| `sol_summaries_reuse` | `source_path: str, manifest_path: str, resolve_modules: bool = False` | `manifest` |
| `ts_summaries_reuse_attempt` | `source_path: str, manifest_path: str, attempt_timeout_ms: int, memory_bound_mb: int = 0` | `report` |
| `rust_summaries_reuse_attempt` | `source_path: str, manifest_path: str, attempt_timeout_ms: int, memory_bound_mb: int = 0` | `report` |
| `sol_summaries_reuse_attempt` | `source_path: str, manifest_path: str, attempt_timeout_ms: int, memory_bound_mb: int = 0` | `report` |
| `ts_summaries_attempt` | `source_path: str, attempt_timeout_ms: int, memory_bound_mb: int = 0` | `report` |
| `rust_summaries_attempt` | `source_path: str, attempt_timeout_ms: int, memory_bound_mb: int = 0` | `report` |
| `sol_summaries_attempt` | `source_path: str, attempt_timeout_ms: int, memory_bound_mb: int = 0` | `report` |
| `ts_summaries_accept` | `source_path: str, manifest_path: str, function: str, resolve_modules: bool = False` | `accept` |
| `rust_summaries_accept` | `source_path: str, manifest_path: str, function: str, resolve_modules: bool = False` | `accept` |
| `sol_summaries_accept` | `source_path: str, manifest_path: str, function: str, resolve_modules: bool = False` | `accept` |
| `suggest_mm_migration` | `code_file: str, language: str, issues_json: str = '[]'` | `migration_hints` |
| `scan_and_fix` | `code_file: str, language: str, spec: str = '', auto_heal: bool = False, heal_output_dir: str = '', domain_hint: str = '', output_format: str = 'json', ctx: Context \| None = None` | `spec_health_issues`, `verification_violations`, `verification_status`, `cross_validation_gaps`, `next_steps`, `migration_hints`, `healed_files`, `heal_errors` |
| `extract_spec_from_code` | `code_file: str, language: str = '', domain_hint: str = '', generate: bool = False, mumei_repo: str = '', ctx: Context \| None = None` | `spec`, `natural_language_spec`, `detected_language`, `warnings`, `code`, `final_spec`, `verified` |
