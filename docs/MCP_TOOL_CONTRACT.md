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
| `get_structured_feedback` | `source_code: str` | `error_type`, `feedback_instruction`, `location`, `reconstruction_loss`, `status` |
| `analyze_contract_conflicts` | `source_code: str` | `agent_artifact_mapping`, `artifact_set`, `callee_atom`, `callee_ensures`, `callee_requires`, `caller_atom`, `caller_ensures`, `caller_requires`, `circular_dependencies`, `conflicts`, `contract_consistency`, `dependency_graph`, `error`, `global_invariant_conflicts`, `human_review_branch`, `mcp_tool`, `mumei_agent`, `mumei_cross_spec`, `success`, `summary`, `violations`, `warnings` |
| `propose_interface_refactoring` | `source_code: str, retry_history: dict | None = None` | `action`, `analysis_summary`, `atom`, `conflict_count`, `error`, `proposals`, `requires` |
| `verify_with_orchestration` | `source_code: str, timeout_ms: int = 30000, enable_cache: bool = True, task_id: str | None = None` | `MUMEI_GENERATION_ID`, `MUMEI_SOLVER_CACHE_KEY`, `MUMEI_SOLVER_CONFIG_FINGERPRINT`, `MUMEI_SOLVER_PROCESS_START_TIME`, `MUMEI_TASK_ID`, `MUMEI_VERIFICATION_TIMEOUT_MS`, `cache_hit`, `cache_key`, `cancel_reason`, `generation_id`, `process_end_time`, `process_start_time`, `proof_certificate`, `raw_certificate`, `raw_report`, `report`, `returncode`, `solver_config_fingerprint`, `source_hash`, `status`, `stderr`, `stdout`, `task_id`, `timeout_ms`, `worker_id` |
| `execute_mm` | `source_code: str, output_name: str = "katana", command: str = "build"` |  |
| `get_inferred_effects` | `source_code: str` |  |
| `get_allowed_effects` | `project_dir: str = "."` | `allowed`, `allowed_effects`, `denied`, `denied_effects`, `source`, `unrestricted` |
| `set_allowed_effects` | `allowed: "list[str] | None" = None, denied: "list[str] | None" = None` | `allowed`, `allowed_effects`, `denied`, `denied_effects`, `message`, `source`, `status` |
| `list_std_catalog` |  | `atoms`, `description`, `effects`, `ensures`, `error`, `import`, `modules`, `path`, `requires`, `signature`, `structs`, `types` |
| `visualize_std_graph` | `format: str = "mermaid"` | `error` |
| `analyze_std_gaps` |  | `candidate_policy`, `core_seed`, `dependency_graph`, `depends_on`, `difficulty`, `error`, `extension_anchor`, `high`, `low`, `max_candidates`, `medium`, `min_candidates`, `name`, `next_implementation_candidates`, `proposals`, `reason`, `selection`, `todo_comments`, `trusted_atoms`, `usage_frequency` |
| `measure_std_health` |  | `atoms`, `details`, `error`, `failed_files`, `file`, `health_score`, `status`, `todo`, `todo_count`, `total_atoms`, `total_files`, `trusted_atoms`, `verified_atoms`, `verified_files`, `verify_unavailable_files` |
| `get_proof_certificate` | `module_path: str` | `bundle_version`, `certificate`, `error`, `looked_at`, `module`, `mumei_version`, `source` |
| `generate_doc` | `source_code: str, format: str = "json"` | `doc`, `error`, `files`, `format`, `stderr`, `stdout` |

## `mumei-agent`

Source: `/home/ubuntu/repos/mumei-agent/agent/mcp_server.py`.

| Tool | Arguments | Documented return keys |
| --- | --- | --- |
| `get_spec_guide_summary` |  | `summary` |
| `get_spec_guidelines` |  | `guidelines` |
| `forge_task` | `task_json: str, mumei_repo: str, dry_run: bool = True, ctx: Context | None = None` | `atoms_added`, `attempts`, `code_length`, `commit_sha`, `dry_run`, `error`, `mcp.tool.dry_run`, `status`, `target_file`, `task_id` |
| `heal_file` | `source_code: str = "", error_report: str = "", code_file: str = "", ctx: Context | None = None` | `attempts`, `healed_code`, `note`, `raw`, `success` |
| `self_correct` | `code_file: str, max_iterations: int = 10, ctx: Context | None = None` | `mcp.tool.max_iterations` |
| `run_nlae_pipeline` | `spec: str, mumei_lean_repo: str = "", work_dir: str = "", no_build: bool = False, multi_agent: bool = False` | `mcp.tool.multi_agent`, `mcp.tool.no_build` |
| `measure_std_health` | `mumei_repo: str` |  |
| `cross_validate` | `spec_file: str, impl_file: str, language: str = ""` | `.go`, `.js`, `.jsx`, `.py`, `.rs`, `.ts`, `.tsx` |
| `propose_forge_tasks` | `mumei_repo: str, max_proposals: int = 3` | `proposals`, `specs`, `todo_comments`, `trusted_atoms` |
| `list_forge_log` | `log_path: str = "forge_log.json"` | `count`, `entries`, `note`, `path` |
| `get_review_queue` | `mumei_repo: str` | `count`, `path`, `queue` |
| `approve_review` | `atom_name: str, reviewer: str, notes: str` | `atom`, `path` |
| `escalate_to_lean` | `atom_name: str` | `atom`, `path` |
| `reject_review` | `atom_name: str, reviewer: str, notes: str` | `atom`, `path` |
| `get_agent_status` |  | `ENABLE_CODE_TO_SPEC`, `ENABLE_DENSE_PROPERTIES`, `ENABLE_LATENT_DEBUG`, `ENABLE_LATENT_PROTOCOL`, `ENABLE_NLAE_MULTI_AGENT`, `INJECT_CORE_AXIOMS`, `PREFER_MCP_GAPS`, `USE_MCP_CLIENT`, `USE_MCP_SAMPLING`, `base_url`, `feature_flags`, `llm_provider`, `mcp_tools`, `model`, `mumei_bin`, `python`, `strategy`, `subcommands` |
| `send_latent_message` | `message: str, context: str = "{}", verify: bool = True` |  |
| `send_latent_message_batch` | `messages: str, verify: bool = False` | `average_transfer_reduction_ratio`, `batch_size`, `error`, `error_type`, `failed`, `index`, `results`, `sent`, `status`, `total_transfer_bytes` |
| `async_send_latent_message` | `message: str, context: str = "{}", verify: bool = True` |  |
| `extract_spec` | `natural_language: str, domain_hint: str = "", generate: bool = False, mumei_repo: str = "", check_contradiction_only: bool = False, ctx: Context | None = None` | `code`, `extraction_attempts`, `extraction_successes`, `spec`, `verified` |
| `check_spec_contradiction` | `natural_language: str, domain_hint: str = "", ctx: Context | None = None` | `check_contradiction_only`, `domain_hint` |
| `check_cross_spec_consistency` | `spec_files: str` | `consistent`, `cross_spec`, `spec_files`, `verification` |
| `check_spec_health` | `source_code: str, mumei_repo: str = ""` |  |
| `validate_nl_spec` | `spec_text: str, use_llm: bool = True, run_mumei: bool = True, domain_hint: str = "", ctx: Context | None = None` |  |
| `validate_nl_spec_multi` | `spec_texts_json: str, domain_hint: str = "", use_llm: bool = True, ctx: Context | None = None` |  |
| `validate_code` | `code: str, language: str, use_llm: bool = True, run_mumei: bool = True, ctx: Context | None = None` |  |
| `validate_foreign_code` | `code: str, language: str, use_llm: bool = True, run_mumei: bool = True, ctx: Context |  |
| `validate_spec_to_code` | `spec: str, code_path: str, language: str | None = None, use_llm: bool = True, run_mumei: bool = True, ctx: Context | None = None` |  |
| `validate_code_to_spec` | `code_path: str, spec_path: str, language: str | None = None, use_llm: bool = True, run_mumei: bool = True, ctx: Context | None = None` |  |
| `verify_conformance` | `spec: str, code_path: str, language: str | None = None, use_llm: bool = True, run_mumei: bool = True, ctx: Context | None = None` | `error`, `hint`, `status` |
| `verify_code_spec_traceability` | `code_file: str, spec_text: str, language: str | None = None, use_llm: bool = True, run_mumei: bool = True, ctx: Context | None = None` | `error`, `hint`, `status` |
| `verify_foreign_code` | `source_code: str, language: str, use_llm: bool = True, run_mumei: bool = True, ctx: Context | None = None` |  |
| `audit_code` | `source_code: str, language: str, domain_hint: str = "", ctx: Context | None = None` | `errors`, `mumei.language`, `success` |
| `suggest_mm_migration` | `code_file: str, language: str, issues_json: str = "[]"` | `issues`, `migration_hints` |
| `scan_and_fix` | `code_file: str, language: str, spec: str = "", auto_heal: bool = False, heal_output_dir: str = "", domain_hint: str = "", output_format: str = "json", ctx: Context | None = None` | `audit`, `audit_schema`, `conformance_verification`, `contract_terms`, `mumei.auto_heal`, `mumei.language`, `next_steps`, `spec_alignment` |
| `extract_spec_from_code` | `code_file: str, language: str = "", domain_hint: str = "", generate: bool = False, mumei_repo: str = "", ctx: Context | None = None` | `code`, `detected_language`, `final_spec`, `mcp.tool.generate`, `natural_language_spec`, `spec`, `verified`, `warnings` |
