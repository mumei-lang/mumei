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
| `set_allowed_effects` | `allowed: "list[str] \| None" = None, denied: "list[str] \| None" = None` |  |
| `list_std_catalog` |  |  |
| `visualize_std_graph` | `format: str = "mermaid"` |  |
| `analyze_std_gaps` |  | `dependency_graph`, `trusted_atoms`, `todo_comments`, `usage_frequency`, `proposals` |
| `measure_std_health` |  | `total_files`, `verified_files`, `failed_files`, `total_atoms`, `verified_atoms`, `trusted_atoms`, `health_score`, `todo_count`, `details` |
| `get_proof_certificate` | `module_path: str` | `certificate`, `error` |
| `generate_doc` | `source_code: str, format: str = "json"` |  |

## `mumei-agent`

Source: `/home/ubuntu/repos/mumei-agent/agent/mcp_server.py`.

| Tool | Arguments | Documented return keys |
| --- | --- | --- |
| `get_spec_guide_summary` |  |  |
| `get_spec_guidelines` |  |  |
| `forge_task` | `task_json: str, mumei_repo: str, dry_run: bool = True, ctx: Context \| None = None` | `task_id`, `status`, `target_file`, `error`, `code_length` |
| `heal_file` | `source_code: str = "", error_report: str = "", code_file: str = "", ctx: Context \| None = None` | `healed_code`, `attempts`, `success`, `error` |
| `self_correct` | `code_file: str, max_iterations: int = 10, ctx: Context \| None = None` |  |
| `run_nlae_pipeline` | `spec: str, mumei_lean_repo: str = "", work_dir: str = "", no_build: bool = False, multi_agent: bool = False` |  |
| `measure_std_health` | `mumei_repo: str` | `total_files`, `verified_files`, `failed_files`, `total_atoms`, `verified_atoms`, `trusted_atoms`, `health_score`, `todo_count`, `details` |
| `cross_validate` | `spec_file: str, impl_file: str, language: str = ""` | `spec_stronger_than_impl`, `impl_stronger_than_spec`, `uncovered_atoms`, `coverage_ratio`, `details` |
| `propose_forge_tasks` | `mumei_repo: str, max_proposals: int = 3` | `proposals`, `specs` |
| `list_forge_log` | `log_path: str = "forge_log.json"` | `entries`, `count` |
| `get_review_queue` | `mumei_repo: str` |  |
| `approve_review` | `atom_name: str, reviewer: str, notes: str` |  |
| `escalate_to_lean` | `atom_name: str` |  |
| `reject_review` | `atom_name: str, reviewer: str, notes: str` |  |
| `get_agent_status` |  |  |
| `send_latent_message` | `message: str, context: str = "{}", verify: bool = True` |  |
| `send_latent_message_batch` | `messages: str, verify: bool = False` |  |
| `async_send_latent_message` | `message: str, context: str = "{}", verify: bool = True` |  |
| `extract_spec` | `natural_language: str, domain_hint: str = "", generate: bool = False, mumei_repo: str = "", check_contradiction_only: bool = False, ctx: Context \| None = None` | `spec`, `code`, `verified` |
| `check_spec_contradiction` | `natural_language: str, domain_hint: str = "", ctx: Context \| None = None` |  |
| `check_cross_spec_consistency` | `spec_files: str` |  |
| `check_spec_health` | `source_code: str, mumei_repo: str = ""` | `contradictions`, `over_constrained`, `vacuous`, `health_score` |
| `validate_nl_spec` | `spec_text: str, use_llm: bool = True, run_mumei: bool = True, domain_hint: str = "", ctx: Context \| None = None` |  |
| `validate_nl_spec_multi` | `spec_texts_json: str, domain_hint: str = "", use_llm: bool = True, ctx: Context \| None = None` |  |
| `validate_code` | `code: str, language: str, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `validate_foreign_code` | `code: str, language: str, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `validate_spec_to_code` | `spec: str, code_path: str, language: str \| None = None, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `validate_code_to_spec` | `code_path: str, spec_path: str, language: str \| None = None, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `verify_conformance` | `spec: str, code_path: str, language: str \| None = None, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `verify_code_spec_traceability` | `code_file: str, spec_text: str, language: str \| None = None, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `verify_foreign_code` | `source_code: str, language: str, use_llm: bool = True, run_mumei: bool = True, ctx: Context \| None = None` |  |
| `audit_code` | `source_code: str, language: str, domain_hint: str = "", ctx: Context \| None = None` |  |
| `suggest_mm_migration` | `code_file: str, language: str, issues_json: str = "[]"` | `migration_hints` |
| `scan_and_fix` | `code_file: str, language: str, spec: str = "", auto_heal: bool = False, heal_output_dir: str = "", domain_hint: str = "", output_format: str = "json", ctx: Context \| None = None` | `spec_health_issues`, `verification_violations`, `verification_status`, `cross_validation_gaps`, `next_steps`, `migration_hints`, `healed_files`, `heal_errors` |
| `extract_spec_from_code` | `code_file: str, language: str = "", domain_hint: str = "", generate: bool = False, mumei_repo: str = "", ctx: Context \| None = None` | `spec`, `natural_language_spec`, `detected_language`, `warnings`, `code`, `final_spec`, `verified` |
