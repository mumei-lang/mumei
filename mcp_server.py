from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import tempfile
import threading
import time
import uuid
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Dict, Optional
from mcp.server.fastmcp import FastMCP

# ws5 / SI-5 Phase 1-C: std/ graph helpers live in std_graph_lib so that
# visualizer/generate_graph.py (and any other consumer) can use them
# without pulling in FastMCP. The underscore-prefixed names are the
# legacy aliases and are re-exported here to keep existing
# `from mcp_server import _scan_std_imports` callers working.
from std_graph_lib import (  # noqa: F401 (re-exported for back-compat)
    _classify_health,
    _collect_trusted_atoms,
    _count_atoms_per_file,
    _render_std_graph_dot,
    _render_std_graph_mermaid,
    _sanitize_node_id,
    _scan_std_imports,
    _trusted_by_file_counts,
)

mcp = FastMCP("Mumei-Forge")

# Module-level session state for effect boundary overrides
_session_effects: dict = {
    "allowed": [],
    "denied": [],
    "source": "default",  # "default" | "mumei.toml" | "session_override"
}


@dataclass
class Z3WorkerContext:
    worker_id: str
    solver_config_fingerprint: str
    process: subprocess.Popen | None = None
    start_time: float = field(default_factory=time.time)


@dataclass
class VerificationTask:
    task_id: str
    source_hash: str
    cache_key: str
    status: str = "pending"
    result: dict | None = None
    cancel_reason: str | None = None
    worker_id: str | None = None
    created_at: float = field(default_factory=time.time)
    completed_at: float | None = None


class VerificationTaskRegistry:
    def __init__(self):
        self._tasks: dict[str, VerificationTask] = {}
        self._cache: dict[str, dict] = {}
        self._lock = threading.RLock()

    def register_task(self, source_hash: str, cache_key: str) -> str:
        task_id = f"verify-{uuid.uuid4().hex}"
        task = VerificationTask(task_id=task_id, source_hash=source_hash, cache_key=cache_key)
        with self._lock:
            self._tasks[task_id] = task
        return task_id

    def get_task(self, task_id: str) -> VerificationTask | None:
        with self._lock:
            return self._tasks.get(task_id)

    def complete_task(self, task_id: str, result: dict, cache_result: bool = True):
        with self._lock:
            task = self._tasks.get(task_id)
            if task is None:
                return
            task.status = "completed"
            task.result = result
            task.completed_at = time.time()
            task.cancel_reason = None
            task.worker_id = None
            if cache_result:
                self._cache[task.cache_key] = result

    def cancel_task(self, task_id: str, reason: str):
        with self._lock:
            task = self._tasks.get(task_id)
            if task is None:
                return
            task.status = "cancelled"
            task.cancel_reason = reason
            task.completed_at = time.time()
            task.worker_id = None

    def mark_running(self, task_id: str, worker_id: str):
        with self._lock:
            task = self._tasks.get(task_id)
            if task is None:
                return
            task.status = "running"
            task.worker_id = worker_id

    def get_cached_result(self, cache_key: str) -> dict | None:
        with self._lock:
            return self._cache.get(cache_key)


class Z3WorkerPool:
    def __init__(self, max_workers: int = 4, timeout_ms: int = 30000, memory_limit_mb: int = 1024):
        self.max_workers = max(1, max_workers)
        self.timeout_ms = timeout_ms
        self.memory_limit_mb = memory_limit_mb
        self._workers = [
            Z3WorkerContext(
                worker_id=f"z3-worker-{index}",
                solver_config_fingerprint=_compute_mcp_solver_config_fingerprint(
                    timeout_ms,
                    memory_limit_mb,
                ),
            )
            for index in range(self.max_workers)
        ]
        self._available = list(self._workers)
        self._busy: dict[str, Z3WorkerContext] = {}
        self._task_workers: dict[str, str] = {}
        self._condition = threading.Condition()

    def acquire_worker(self) -> Z3WorkerContext:
        with self._condition:
            while not self._available:
                self._condition.wait()
            worker = self._available.pop(0)
            worker.start_time = time.time()
            self._busy[worker.worker_id] = worker
            return worker

    def release_worker(self, worker_id: str):
        with self._condition:
            worker = self._busy.pop(worker_id, None)
            if worker is None:
                return
            if worker.process is not None and worker.process.poll() is None:
                worker.process.terminate()
            worker.process = None
            self._task_workers = {
                task_id: mapped_worker_id
                for task_id, mapped_worker_id in self._task_workers.items()
                if mapped_worker_id != worker_id
            }
            self._available.append(worker)
            self._condition.notify()

    def cancel_task(self, task_id: str):
        with self._condition:
            worker_id = self._task_workers.get(task_id)
            if worker_id is None:
                return False
            worker = self._busy.get(worker_id)
            if worker is None or worker.process is None:
                return False
            if worker.process.poll() is None:
                worker.process.terminate()
            return True

    def bind_task(self, task_id: str, worker_id: str):
        with self._condition:
            self._task_workers[task_id] = worker_id

    def shutdown(self):
        with self._condition:
            for worker in self._workers:
                if worker.process is not None and worker.process.poll() is None:
                    worker.process.terminate()
                worker.process = None
            self._available = list(self._workers)
            self._busy.clear()
            self._task_workers.clear()
            self._condition.notify_all()


def _detect_mcp_solver_features(source_code: str) -> dict[str, bool]:
    return {
        "has_string_constraints": bool(
            re.search(r"\b(Str|string|String)\b|starts_with|ends_with|contains", source_code)
        ),
        "has_array_forall": bool(
            re.search(r"forall\s*\([^)]*\)[^\n;]*\[", source_code, re.DOTALL)
        ),
    }


def _compute_mcp_solver_config_fingerprint(
    timeout_ms: int,
    memory_limit_mb: int,
    mbqi: bool = True,
    has_string_constraints: bool = False,
    has_array_forall: bool = False,
    enable_spurious_detection: bool = True,
) -> str:
    payload = json.dumps(
        {
            "engine": "mumei-z3",
            "timeout_ms": timeout_ms,
            "memory_limit_mb": memory_limit_mb,
            "smt.mbqi": mbqi,
            "string_constraints": has_string_constraints,
            "array_forall": has_array_forall,
            "spurious_detection": enable_spurious_detection,
        },
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _limit_worker_memory(memory_limit_mb: int):
    if os.name != "posix":
        return None

    def _set_limit():
        try:
            import resource

            limit_bytes = memory_limit_mb * 1024 * 1024
            resource.setrlimit(resource.RLIMIT_AS, (limit_bytes, limit_bytes))
        except (ImportError, OSError, ValueError):
            pass

    return _set_limit


def _resume_task_validation_error(
    task_id: str | None,
    resumed_task: VerificationTask | None,
    source_hash: str,
    cache_key: str,
) -> dict | None:
    if task_id is None:
        return None
    if resumed_task is None:
        return {"status": "error", "task_id": task_id, "error": "task_id not found"}
    if resumed_task.source_hash != source_hash or resumed_task.cache_key != cache_key:
        return {
            "status": "error",
            "task_id": task_id,
            "error": "task_id does not match source_hash or solver configuration",
            "expected_source_hash": resumed_task.source_hash,
            "requested_source_hash": source_hash,
            "expected_cache_key": resumed_task.cache_key,
            "requested_cache_key": cache_key,
        }
    return None


_task_registry = VerificationTaskRegistry()
_z3_worker_pool = Z3WorkerPool()

def _format_semantic_feedback(report_json: str) -> str:
    """Parse report.json and format semantic_feedback into a readable section.
    Returns empty string if no semantic_feedback is present (backward compatible).
    Includes a machine_readable JSON block for AI agent consumption."""
    try:
        report = json.loads(report_json)
    except (json.JSONDecodeError, TypeError):
        return ""

    feedback = report.get("semantic_feedback")
    if not feedback:
        return ""

    parts = ["### Semantic Feedback"]
    violated = feedback.get("violated_constraints", [])
    for vc in violated:
        param = vc.get("param", "?")
        typ = vc.get("type", "")
        value = vc.get("value", "?")
        constraint = vc.get("constraint", "")
        explanation = vc.get("explanation", "")
        suggestion = vc.get("suggestion", "")
        parts.append(f"- **{param}** (type `{typ}`, value `{value}`): constraint `{constraint}` violated")
        if explanation:
            parts.append(f"  - {explanation}")
        if suggestion:
            parts.append(f"  - Suggestion: {suggestion}")
        # Sub-constraint decomposition display
        sub_constraints = vc.get("sub_constraints", [])
        if sub_constraints:
            for sc in sub_constraints:
                sc_idx = sc.get("index", 0)
                sc_total = len(sub_constraints)
                sc_raw = sc.get("raw", "")
                sc_satisfied = sc.get("satisfied", False)
                sc_explanation = sc.get("explanation", "")
                status_icon = "\u2705" if sc_satisfied else "\u274c"
                status_text = "satisfied" if sc_satisfied else "violated"
                line = f"  - Sub-constraint [{sc_idx + 1}/{sc_total}] `{sc_raw}`: {status_icon} {status_text}"
                if not sc_satisfied and sc_explanation:
                    line += f" \u2014 {sc_explanation}"
                parts.append(line)

    # Unsat core: conflicting constraints (contradiction detection)
    conflicting = feedback.get("conflicting_constraints", [])
    if conflicting:
        parts.append("\n**Conflicting Constraints (Unsat Core):**")
        for c in conflicting:
            parts.append(f"- {c}")
        explanation = feedback.get("explanation", "")
        if explanation:
            parts.append(f"\n  {explanation}")

    # Linearity violations
    violations = feedback.get("violations", [])
    for v in violations:
        desc = v.get("description", "")
        expl = v.get("explanation", "")
        if desc:
            parts.append(f"- {desc}")
        if expl:
            parts.append(f"  - {expl}")

    # Division-by-zero specific
    # Check both top-level report and semantic_feedback sub-object for failure_type,
    # since build_division_by_zero_feedback embeds failure_type in the feedback object.
    report_failure_type = report.get("failure_type", "")
    feedback_failure_type = feedback.get("failure_type", "")
    effective_failure_type = report_failure_type or feedback_failure_type

    if effective_failure_type == "division_by_zero":
        # counter_example may be in feedback sub-object (from build_division_by_zero_feedback)
        ce = feedback.get("counter_example", {})
        if ce:
            parts.append(f"- Counter-example: dividend = {ce.get('dividend', '?')}, divisor = {ce.get('divisor', '?')}")

    # Effect violations
    if effective_failure_type == "effect_not_allowed":
        parts.append(f"- Attempted effect: `{feedback.get('attempted_effect', '?')}`")
        parts.append(f"- Allowed effects: {feedback.get('allowed_effects', [])}")
        parts.append(f"- Missing effects: {feedback.get('missing_effects', [])}")

    # Data flow display (Feature 2e)
    data_flow = feedback.get("data_flow", [])
    if data_flow:
        parts.append("\n**Data Flow:**")
        for entry in data_flow:
            step = entry.get("step", "?")
            line = entry.get("line", 0)
            col = entry.get("col", 0)
            desc = entry.get("description", "")
            constraint = entry.get("constraint", "")
            flow_line = f"- [{step}] line {line}:{col}"
            if desc:
                flow_line += f" — {desc}"
            if constraint:
                flow_line += f" (constraint: `{constraint}`)"
            parts.append(flow_line)

    # Related locations display (Feature 3g)
    related_locations = feedback.get("related_locations", [])
    if related_locations:
        parts.append("\n**Related Locations:**")
        for loc in related_locations:
            loc_file = loc.get("file", "?")
            loc_line = loc.get("line", 0)
            loc_label = loc.get("label", "")
            parts.append(f"- {loc_file}:{loc_line} — {loc_label}")

    ctx = feedback.get("context", {})
    if ctx:
        parts.append("\n**Context:**")
        if ctx.get("requires"):
            parts.append(f"- requires: `{ctx['requires']}`")
        if ctx.get("ensures"):
            parts.append(f"- ensures: `{ctx['ensures']}`")

    if effective_failure_type:
        parts.append(f"\n**Failure type:** `{effective_failure_type}`")

    suggestion = report.get("suggestion", "")
    if suggestion:
        parts.append(f"**Suggestion:** {suggestion}")

    span = report.get("span")
    if span:
        parts.append(f"**Location:** {span.get('file', '?')}:{span.get('line', '?')}:{span.get('col', '?')}")

    # Machine-readable section for AI agents
    machine_readable = _build_machine_readable(report, feedback)
    if machine_readable:
        parts.append(f"\n### Machine Readable\n```json\n{json.dumps(machine_readable, indent=2)}\n```")

    return "\n".join(parts)


def _build_machine_readable(report: dict, feedback: dict) -> "dict | None":
    """Build a machine-readable JSON block from report and feedback for AI agents."""
    failure_type = report.get("failure_type", "")
    if not failure_type:
        return None

    result = {
        "failure_type": failure_type,
        "atom": report.get("atom", ""),
    }

    span = report.get("span", {})
    if span:
        result["file"] = span.get("file", "")
        result["line"] = span.get("line", 0)

    violated = feedback.get("violated_constraints", [])
    if violated:
        actions = []
        for vc in violated:
            action = {
                "action": "fix_constraint",
                "param": vc.get("param", ""),
                "current_value": vc.get("value", ""),
                "constraint": vc.get("constraint", ""),
            }
            # Include sub_constraints in machine-readable output
            if vc.get("sub_constraints"):
                action["sub_constraints"] = vc["sub_constraints"]
            actions.append(action)
        result["actions"] = actions

    if feedback.get("counter_example"):
        result["counter_example"] = feedback["counter_example"]

    conflicting = feedback.get("conflicting_constraints", [])
    if conflicting:
        result["conflicting_constraints"] = conflicting
        result["raw_unsat_core"] = feedback.get("raw_unsat_core", [])

    # Include data_flow in machine-readable output (Feature 2e)
    data_flow = feedback.get("data_flow", [])
    if data_flow:
        result["data_flow"] = data_flow

    # Include related_locations in machine-readable output (Feature 3g)
    related_locations = feedback.get("related_locations", [])
    if related_locations:
        result["related_locations"] = related_locations

    result["suggestion"] = report.get("suggestion", "")
    return result


def _normalize_spec_metadata(spec_metadata: Optional[Dict[str, str]] = None) -> dict:
    if spec_metadata is None:
        return {}
    if isinstance(spec_metadata, str):
        try:
            parsed = json.loads(spec_metadata)
        except json.JSONDecodeError:
            return {"value": spec_metadata}
        if isinstance(parsed, dict):
            spec_metadata = parsed
        else:
            return {"value": str(parsed)}
    if not isinstance(spec_metadata, dict):
        return {"value": str(spec_metadata)}
    return {str(key): str(value) for key, value in spec_metadata.items()}


def _traceability_payload(
    source_code: str,
    trace_id: Optional[str] = None,
    spec_metadata: Optional[Dict[str, str]] = None,
) -> dict:
    normalized_metadata = _normalize_spec_metadata(spec_metadata)
    requires = re.findall(r"\brequires\s*:\s*([^;]*);", source_code, re.S)
    ensures = re.findall(r"\bensures\s*:\s*([^;]*);", source_code, re.S)
    hasher = hashlib.sha256()
    hasher.update((trace_id or "").encode("utf-8"))
    for key, value in sorted(normalized_metadata.items()):
        hasher.update(key.encode("utf-8"))
        hasher.update(b"=")
        hasher.update(value.encode("utf-8"))
        hasher.update(b";")
    hasher.update(" && ".join(item.strip() for item in requires).encode("utf-8"))
    hasher.update(" && ".join(item.strip() for item in ensures).encode("utf-8"))
    covered = sum([
        bool((trace_id or "").strip()),
        bool(normalized_metadata),
        any(item.strip() and item.strip() != "true" for item in requires),
        any(item.strip() and item.strip() != "true" for item in ensures),
    ])
    return {
        "trace_id": trace_id or None,
        "spec_metadata": normalized_metadata,
        "traceability_hash": hasher.hexdigest(),
        "traceability_coverage": covered / 4.0,
    }


def _traceability_env(trace_payload: dict) -> dict:
    env = os.environ.copy()
    if trace_payload["trace_id"]:
        env["MUMEI_TRACE_ID"] = trace_payload["trace_id"]
    env["MUMEI_SPEC_METADATA"] = json.dumps(trace_payload["spec_metadata"], sort_keys=True)
    return env


def _format_traceability_feedback(trace_payload: dict) -> str:
    return "### Traceability\n```json\n" + json.dumps(trace_payload, indent=2, sort_keys=True) + "\n```"


def _format_effect_feedback(report_json: str) -> str:
    """Format effect-specific violation feedback from report.json.
    Returns empty string if no effect violation is present.
    Handles both mismatch (required_effect/source_operation) and
    propagation (caller/callee/missing_effects) violation structures."""
    try:
        report = json.loads(report_json)
    except (json.JSONDecodeError, TypeError):
        return ""

    effect_violation = report.get("effect_violation")
    if not effect_violation:
        return ""

    parts = ["### Effect Violation Details"]
    violation_type = report.get("violation_type", "")

    if violation_type == "effect_propagation":
        # save_effect_propagation_report structure: caller/callee/missing_effects
        parts.append(f"- **Caller atom:** `{effect_violation.get('caller', '?')}`")
        parts.append(f"- **Callee atom:** `{effect_violation.get('callee', '?')}`")
        parts.append(f"- **Caller declared effects:** {effect_violation.get('caller_effects', [])}")
        parts.append(f"- **Callee required effects:** {effect_violation.get('callee_effects', [])}")
        parts.append(f"- **Missing effects:** {effect_violation.get('missing_effects', [])}")
    else:
        # save_effect_violation_report structure (effect_mismatch): required_effect/source_operation
        parts.append(f"- **Declared effects:** {effect_violation.get('declared_effects', [])}")
        parts.append(f"- **Required effect:** `{effect_violation.get('required_effect', '?')}`")
        parts.append(f"- **Source operation:** `{effect_violation.get('source_operation', '?')}`")

    suggested_fixes = effect_violation.get("suggested_fixes", [])
    if suggested_fixes:
        parts.append("\n**Suggested Fixes:**")
        for fix in suggested_fixes:
            parts.append(f"- {fix}")

    resolution_paths = effect_violation.get("resolution_paths", [])
    if resolution_paths:
        parts.append("\n**Resolution Paths:**")
        for rp in resolution_paths:
            parts.append(f"- **{rp.get('strategy', '?')}**: {rp.get('description', '')}")

    return "\n".join(parts)


@mcp.tool()
def forge_blade(
    source_code: str,
    output_name: str = "katana",
    trace_id: Optional[str] = None,
    spec_metadata: Optional[Dict[str, str]] = None,
) -> str:
    """
    Verify Mumei code and generate LLVM IR output.
    The build command writes report.json to the -o directory, so reports are isolated per request.
    Optional trace_id/spec_metadata are forwarded into proof certificates.
    """
    root_dir = Path(__file__).parent.absolute()
    trace_payload = _traceability_payload(source_code, trace_id, spec_metadata)

    # 1. Create fully isolated temp directory per request
    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = Path(tmpdir)
        source_path = tmp_path / "input.mm"
        source_path.write_text(source_code, encoding="utf-8")

        # 2. Run compiler (output to temp directory)
        output_base = tmp_path / output_name

        result = subprocess.run(
            ["cargo", "run", "--", "build", str(source_path), "-o", str(output_base)],
            cwd=root_dir,
            capture_output=True,
            text=True,
            env=_traceability_env(trace_payload),
        )

        response_parts = [_format_traceability_feedback(trace_payload)]

        # Inject effect boundary context if restricted
        effects_ctx = json.loads(get_allowed_effects(str(root_dir)))
        if not effects_ctx.get("unrestricted", True):
            response_parts.append(
                f"### Effect Boundary\n{effects_ctx['summary']}\n"
            )

        # report.json is written to output_dir (parent of -o path) = tmp_path
        report_file = tmp_path / "report.json"
        if report_file.exists():
            report_data = report_file.read_text(encoding="utf-8")
            response_parts.append(f"### Verification Report\n```json\n{report_data}\n```")
            # Include semantic feedback section (always present for AI agents)
            sf_section = _format_semantic_feedback(report_data)
            if sf_section:
                response_parts.append(sf_section)
            else:
                response_parts.append(
                    '### Semantic Feedback\n'
                    '```json\n{"status": "all_constraints_satisfied"}\n```'
                )
            # Include effect-specific feedback if present
            ef_section = _format_effect_feedback(report_data)
            if ef_section:
                response_parts.append(ef_section)
        else:
            response_parts.append(
                '### Semantic Feedback\n'
                '```json\n{"status": "no_report_available"}\n```'
            )

        if result.returncode == 0:
            response_parts.insert(0, f"Forge succeeded: '{output_name}'")
            # Collect generated per-atom LLVM IR artifacts (e.g. katana_increment.ll)
            for ll_file in sorted(tmp_path.glob(f"{output_name}*.ll")):
                content = ll_file.read_text(encoding="utf-8")
                response_parts.append(f"\n### Generated: {ll_file.name}\n```llvm\n{content}\n```")

            return "\n".join(response_parts)
        else:
            # On failure: return evidence (report) and error log together
            response_parts.insert(0, f"Forge failed: logical flaw detected.")
            if result.stderr:
                response_parts.append(f"\n### Error Details\n{result.stderr}")

            return "\n".join(response_parts)

@mcp.tool()
def validate_logic(
    source_code: str,
    trace_id: Optional[str] = None,
    spec_metadata: Optional[Dict[str, str]] = None,
) -> str:
    """
    Run formal verification (Z3) only on Mumei code.
    No code generation — returns verification results and counter-examples.
    Used as the verification step when AI iteratively fixes .mm code.

    Uses --report-dir to write report.json directly into a per-request temp
    directory, making concurrent calls safe.
    Optional trace_id/spec_metadata are forwarded into proof certificates.
    """
    root_dir = Path(__file__).parent.absolute()
    trace_payload = _traceability_payload(source_code, trace_id, spec_metadata)

    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = Path(tmpdir)
        source_path = tmp_path / "input.mm"
        source_path.write_text(source_code, encoding="utf-8")

        # Run mumei verify with --report-dir to write report.json directly
        # into the per-request temp directory (concurrent-safe).
        result = subprocess.run(
            ["cargo", "run", "--", "verify",
             "--report-dir", str(tmp_path),
             str(source_path)],
            cwd=root_dir,
            capture_output=True,
            text=True,
            env=_traceability_env(trace_payload),
        )

        response_parts = [_format_traceability_feedback(trace_payload)]

        # Inject effect boundary context if restricted
        effects_ctx = json.loads(get_allowed_effects(str(root_dir)))
        if not effects_ctx.get("unrestricted", True):
            response_parts.append(
                f"### Effect Boundary\n{effects_ctx['summary']}\n"
            )

        report_file = tmp_path / "report.json"
        if report_file.exists():
            report_data = report_file.read_text(encoding="utf-8")
            response_parts.append(
                f"### Verification Report\n```json\n{report_data}\n```"
            )
            # Include semantic feedback section (always present for AI agents)
            sf_section = _format_semantic_feedback(report_data)
            if sf_section:
                response_parts.append(sf_section)
            else:
                # Even on success, include a semantic_feedback status for AI agents
                response_parts.append(
                    '### Semantic Feedback\n'
                    '```json\n{"status": "all_constraints_satisfied"}\n```'
                )
            # Include effect-specific feedback if present
            ef_section = _format_effect_feedback(report_data)
            if ef_section:
                response_parts.append(ef_section)
        else:
            # No report file — still include semantic feedback status
            response_parts.append(
                '### Semantic Feedback\n'
                '```json\n{"status": "no_report_available"}\n```'
            )

        # Extract Z3 counter-example info from stderr
        if result.stderr:
            counterexamples = re.findall(
                r'Counter-example:.*', result.stderr
            )
            if counterexamples:
                response_parts.append("### Z3 Counter-examples")
                for ce in counterexamples:
                    response_parts.append(f"- `{ce.strip()}`")

        if result.returncode == 0:
            response_parts.insert(
                0, "Verification passed: no logical flaws detected."
            )
        else:
            response_parts.insert(
                0, "Verification failed: logical flaw detected."
            )
            if result.stderr:
                response_parts.append(
                    f"\n### Error Details\n```\n{result.stderr}\n```"
                )

        return "\n".join(response_parts)


@mcp.tool()
def verify_with_orchestration(
    source_code: str,
    timeout_ms: int = 30000,
    enable_cache: bool = True,
    task_id: str | None = None,
) -> str:
    """
    Run verification through an orchestration-aware Z3 worker pool.
    This tool leaves validate_logic/forge_blade unchanged and adds task IDs,
    cache isolation, bounded parallelism, timeout cancellation, and process metadata.
    """
    root_dir = Path(__file__).parent.absolute()
    timeout_ms = max(1, timeout_ms)
    source_hash = hashlib.sha256(source_code.encode("utf-8")).hexdigest()
    solver_features = _detect_mcp_solver_features(source_code)
    solver_fingerprint = _compute_mcp_solver_config_fingerprint(
        timeout_ms,
        _z3_worker_pool.memory_limit_mb,
        mbqi=not solver_features["has_array_forall"],
        has_string_constraints=solver_features["has_string_constraints"],
        has_array_forall=solver_features["has_array_forall"],
    )
    cache_key = hashlib.sha256(
        f"{source_hash}:{solver_fingerprint}".encode("utf-8")
    ).hexdigest()

    resumed_task = _task_registry.get_task(task_id) if task_id else None
    resume_error = _resume_task_validation_error(task_id, resumed_task, source_hash, cache_key)
    if resume_error is not None:
        return json.dumps(resume_error, ensure_ascii=False, indent=2)

    if resumed_task is not None:
        if resumed_task.status == "completed" and resumed_task.result is not None:
            response = dict(resumed_task.result)
            response["resumed"] = True
            return json.dumps(response, ensure_ascii=False, indent=2)
        if resumed_task.status == "cancelled":
            return json.dumps(
                {
                    "status": "cancelled",
                    "task_id": resumed_task.task_id,
                    "cache_key": resumed_task.cache_key,
                    "cancel_reason": resumed_task.cancel_reason,
                },
                ensure_ascii=False,
                indent=2,
            )

    if enable_cache:
        cached_result = _task_registry.get_cached_result(cache_key)
        if cached_result is not None:
            response = dict(cached_result)
            response["cache_hit"] = True
            response["cache_key"] = cache_key
            return json.dumps(response, ensure_ascii=False, indent=2)

    if resumed_task is not None:
        active_task_id = resumed_task.task_id
    else:
        active_task_id = _task_registry.register_task(source_hash, cache_key)

    worker = _z3_worker_pool.acquire_worker()
    worker.solver_config_fingerprint = solver_fingerprint
    _z3_worker_pool.bind_task(active_task_id, worker.worker_id)
    _task_registry.mark_running(active_task_id, worker.worker_id)
    generation_id = f"generation-{uuid.uuid4().hex}"
    process_start_time = datetime.now(timezone.utc).isoformat()

    try:
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = Path(tmpdir)
            source_path = tmp_path / "input.mm"
            source_path.write_text(source_code, encoding="utf-8")
            env = os.environ.copy()
            env.update(
                {
                    "MUMEI_TASK_ID": active_task_id,
                    "MUMEI_GENERATION_ID": generation_id,
                    "MUMEI_SOLVER_CONFIG_FINGERPRINT": solver_fingerprint,
                    "MUMEI_SOLVER_CACHE_KEY": cache_key,
                    "MUMEI_VERIFICATION_TIMEOUT_MS": str(timeout_ms),
                    "MUMEI_SOLVER_PROCESS_START_TIME": process_start_time,
                }
            )
            process = subprocess.Popen(
                [
                    "cargo",
                    "run",
                    "--",
                    "verify",
                    "--proof-cert",
                    "--output",
                    str(tmp_path / "input.proof.json"),
                    "--report-dir",
                    str(tmp_path),
                    str(source_path),
                ],
                cwd=root_dir,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                env=env,
                preexec_fn=_limit_worker_memory(_z3_worker_pool.memory_limit_mb),
            )
            worker.process = process
            try:
                stdout, stderr = process.communicate(timeout=timeout_ms / 1000)
            except subprocess.TimeoutExpired:
                _z3_worker_pool.cancel_task(active_task_id)
                try:
                    stdout, stderr = process.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    stdout, stderr = process.communicate()
                reason = f"timeout after {timeout_ms}ms"
                _task_registry.cancel_task(active_task_id, reason)
                result_payload = {
                    "status": "cancelled",
                    "task_id": active_task_id,
                    "generation_id": generation_id,
                    "worker_id": worker.worker_id,
                    "source_hash": source_hash,
                    "cache_key": cache_key,
                    "solver_config_fingerprint": solver_fingerprint,
                    "timeout_ms": timeout_ms,
                    "cancel_reason": reason,
                    "stdout": stdout,
                    "stderr": stderr,
                    "process_start_time": process_start_time,
                    "process_end_time": datetime.now(timezone.utc).isoformat(),
                }
                return json.dumps(result_payload, ensure_ascii=False, indent=2)

            report_file = tmp_path / "report.json"
            report_data = None
            if report_file.exists():
                report_text = report_file.read_text(encoding="utf-8")
                try:
                    report_data = json.loads(report_text)
                except json.JSONDecodeError:
                    report_data = {"raw_report": report_text}

            certificate_file = tmp_path / "input.proof.json"
            certificate_data = None
            if certificate_file.exists():
                certificate_text = certificate_file.read_text(encoding="utf-8")
                try:
                    certificate_data = json.loads(certificate_text)
                except json.JSONDecodeError:
                    certificate_data = {"raw_certificate": certificate_text}

            result_payload = {
                "status": "passed" if process.returncode == 0 else "failed",
                "task_id": active_task_id,
                "generation_id": generation_id,
                "worker_id": worker.worker_id,
                "source_hash": source_hash,
                "cache_key": cache_key,
                "solver_config_fingerprint": solver_fingerprint,
                "timeout_ms": timeout_ms,
                "returncode": process.returncode,
                "cache_hit": False,
                "report": report_data,
                "proof_certificate": certificate_data,
                "stdout": stdout,
                "stderr": stderr,
                "process_start_time": process_start_time,
                "process_end_time": datetime.now(timezone.utc).isoformat(),
            }
            _task_registry.complete_task(active_task_id, result_payload, cache_result=enable_cache)
            return json.dumps(result_payload, ensure_ascii=False, indent=2)
    finally:
        _z3_worker_pool.release_worker(worker.worker_id)


@mcp.tool()
def execute_mm(
    source_code: str,
    output_name: str = "katana",
    command: str = "build",
) -> str:
    """
    Compile and execute Mumei code.
    command: "build" (default) for full build, "verify" for verification only, "check" for syntax check only.
    Returns build results, generated code, and verification report.

    Note: "build" and "verify" are concurrent-safe (report isolated via -o / --report-dir).
    "check" still writes report.json to cwd, so concurrent calls may race.
    """
    root_dir = Path(__file__).parent.absolute()

    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = Path(tmpdir)
        source_path = tmp_path / "input.mm"
        source_path.write_text(source_code, encoding="utf-8")

        output_base = tmp_path / output_name

        # Validate command against allowlist
        allowed_commands = {"build", "verify", "check"}
        if command not in allowed_commands:
            return f"Invalid command: '{command}'. Allowed: {', '.join(sorted(allowed_commands))}"

        # Build command arguments
        cmd_args = ["cargo", "run", "--", command, str(source_path)]
        if command == "build":
            cmd_args.extend(["-o", str(output_base)])
        elif command == "verify":
            # Use --report-dir to write report.json directly into the
            # per-request temp directory (concurrent-safe).
            cmd_args.extend(["--report-dir", str(tmp_path)])

        result = subprocess.run(
            cmd_args,
            cwd=root_dir,
            capture_output=True,
            text=True,
            timeout=300,
        )

        response_parts = []

        # For "build", report.json is written to tmp_path (via -o).
        # For "verify", report.json is written to tmp_path (via --report-dir).
        # For "check", report.json is still written to cwd (no --report-dir yet).
        report_file = tmp_path / "report.json"
        if not report_file.exists() and command == "check":
            cwd_report = root_dir / "report.json"
            if cwd_report.exists():
                try:
                    shutil.move(str(cwd_report), str(report_file))
                except OSError:
                    pass
        if report_file.exists():
            report_data = report_file.read_text(encoding="utf-8")
            response_parts.append(
                f"### Verification Report\n```json\n{report_data}\n```"
            )
            # Include semantic feedback section (always present for AI agents)
            sf_section = _format_semantic_feedback(report_data)
            if sf_section:
                response_parts.append(sf_section)
            else:
                response_parts.append(
                    '### Semantic Feedback\n'
                    '```json\n{"status": "all_constraints_satisfied"}\n```'
                )
            # Include effect-specific feedback if present
            ef_section = _format_effect_feedback(report_data)
            if ef_section:
                response_parts.append(ef_section)
        else:
            response_parts.append(
                '### Semantic Feedback\n'
                '```json\n{"status": "no_report_available"}\n```'
            )

        if result.returncode == 0:
            response_parts.insert(0, f"{command} succeeded: '{output_name}'")
            # Collect generated per-atom LLVM IR artifacts (e.g. katana_increment.ll)
            for ll_file in sorted(tmp_path.glob(f"{output_name}*.ll")):
                content = ll_file.read_text(encoding="utf-8")
                response_parts.append(
                    f"\n### Generated: {ll_file.name}"
                    f"\n```llvm\n{content}\n```"
                )
        else:
            response_parts.insert(0, f"{command} failed")
            if result.stderr:
                response_parts.append(
                    f"\n### Error Details\n```\n{result.stderr}\n```"
                )
            if result.stdout:
                response_parts.append(
                    f"\n### Standard Output\n```\n{result.stdout}\n```"
                )

        return "\n".join(response_parts)


@mcp.tool()
def get_inferred_effects(source_code: str) -> str:
    """
    Pre-check: Infer what effects are required for the given Mumei code,
    WITHOUT running full verification. Returns a JSON analysis of:
    - declared_effects: effects explicitly annotated on each atom
    - inferred_effects: effects inferred from call graph analysis
    - missing_effects: effects that should be added
    - suggestion: suggested effects: [...] annotation

    Use this BEFORE writing code to check what permissions (effects) your
    code will need. This enables AI to "check its own permissions" before
    committing to a code path.
    """
    root_dir = Path(__file__).parent.absolute()

    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = Path(tmpdir)
        source_path = tmp_path / "input.mm"
        source_path.write_text(source_code, encoding="utf-8")

        result = subprocess.run(
            ["cargo", "run", "--", "infer-effects", str(source_path)],
            cwd=root_dir,
            capture_output=True,
            text=True,
            timeout=60,
        )

        if result.returncode == 0 and result.stdout.strip():
            try:
                analysis = json.loads(result.stdout)
                formatted = json.dumps(analysis, indent=2, ensure_ascii=False)
                return f"### Effect Analysis\n```json\n{formatted}\n```"
            except json.JSONDecodeError:
                return f"Effect inference completed but output was not valid JSON:\n{result.stdout}"
        else:
            parts = ["Effect inference failed."]
            if result.stderr:
                parts.append(f"\n### Error Details\n```\n{result.stderr}\n```")
            return "\n".join(parts)


@mcp.tool()
def get_allowed_effects(project_dir: str = ".") -> str:
    """
    Returns the current effect boundary for this session.
    AI agents should call this BEFORE generating any .mm code
    to understand which effects (I/O, Network, FileWrite, etc.) are permitted.

    The boundary is determined by:
    1. Session override (set via set_allowed_effects)
    2. mumei.toml [effects] section
    3. Default: all effects allowed (no restrictions)

    Returns structured JSON with allowed/denied effect sets and
    a natural language summary suitable for system prompt injection.
    """
    root_dir = Path(project_dir).absolute()

    # Check session override first
    if _session_effects["source"] == "session_override":
        effects = _session_effects
    else:
        # Read from mumei.toml
        toml_path = root_dir / "mumei.toml"
        effects = {"allowed": [], "denied": [], "source": "default"}
        if toml_path.exists():
            try:
                import tomllib

                with open(toml_path, "rb") as f:
                    config = tomllib.load(f)
                if "effects" in config:
                    effects["allowed"] = config["effects"].get("allowed", [])
                    effects["denied"] = config["effects"].get("denied", [])
                    effects["source"] = "mumei.toml"
            except Exception:
                pass

    # Build response
    response = {
        "allowed_effects": effects["allowed"],
        "denied_effects": effects["denied"],
        "source": effects["source"],
        "unrestricted": len(effects["allowed"]) == 0
        and len(effects["denied"]) == 0,
    }

    # Natural language summary for AI context injection
    if response["unrestricted"]:
        summary = "All effects are currently permitted. No restrictions."
    else:
        parts = []
        if effects["allowed"]:
            parts.append(f"Allowed effects: {effects['allowed']}")
        if effects["denied"]:
            parts.append(f"Denied effects: {effects['denied']}")
        parts.append(
            "Code using effects outside this boundary will be rejected by Z3 verification."
        )
        summary = " ".join(parts)

    response["summary"] = summary

    return json.dumps(response, indent=2)


@mcp.tool()
def set_allowed_effects(
    allowed: "list[str] | None" = None, denied: "list[str] | None" = None
) -> str:
    """
    Override the effect boundary for the current MCP session.
    This takes precedence over mumei.toml settings.

    Use this to restrict or expand the AI agent's capabilities dynamically.
    Example: set_allowed_effects(allowed=["Log", "FileRead"], denied=["Network"])

    To reset to mumei.toml defaults, call with empty lists.
    """
    global _session_effects

    if allowed is None:
        allowed = []
    if denied is None:
        denied = []

    if not allowed and not denied:
        _session_effects = {"allowed": [], "denied": [], "source": "default"}
        return "Effect boundary reset to project defaults (mumei.toml or unrestricted)."

    _session_effects = {
        "allowed": allowed,
        "denied": denied,
        "source": "session_override",
    }

    return json.dumps(
        {
            "status": "updated",
            "allowed_effects": allowed,
            "denied_effects": denied,
            "message": (
                f"Session effect boundary set. Only {allowed} effects are permitted."
                if allowed
                else f"Effects {denied} are now denied."
            ),
        },
        indent=2,
    )


@mcp.tool()
def list_std_catalog() -> str:
    """List all available standard library modules and their verified atoms.
    AI agents should call this to discover reusable verified components
    before generating new code from scratch.
    Returns JSON with module names, atom signatures, refinement types,
    requires/ensures contracts, effects, and module descriptions."""
    std_dir = Path(__file__).parent.absolute() / "std"
    if not std_dir.exists():
        return json.dumps({"error": "std/ directory not found"})

    modules = []
    for mm_file in sorted(std_dir.rglob("*.mm")):
        rel_path = mm_file.relative_to(std_dir.parent)
        import_path = str(rel_path).replace(".mm", "").replace("\\", "/")
        content = mm_file.read_text(encoding="utf-8")
        lines = content.splitlines()

        # Extract module description from leading comment block
        description = ""
        desc_lines: list[str] = []
        for line in lines:
            stripped = line.strip()
            if stripped.startswith("//"):
                desc_lines.append(stripped.lstrip("/ ").strip())
            elif stripped == "":
                if desc_lines:
                    continue  # skip blank lines within comment block
            else:
                break
        if desc_lines:
            # Use the first meaningful non-separator line as description
            for dl in desc_lines:
                if dl and not all(c in "=-" for c in dl):
                    description = dl
                    break

        types: list[str] = []
        atoms: list[dict] = []
        structs: list[str] = []
        effects: list[str] = []

        i = 0
        while i < len(lines):
            stripped = lines[i].strip()

            # Type definitions
            if re.match(r"^type \w+", stripped):
                types.append(stripped.rstrip(";"))
                i += 1
                continue

            # Struct definitions
            if re.match(r"^struct \w+", stripped):
                structs.append(stripped.rstrip("{").strip())
                i += 1
                continue

            # Effect definitions: all forms
            #   - Non-parameterized: effect FileRead;
            #   - Parameterized: effect HttpGet(url: Str);
            #   - Parameterized with constraint: effect SafeFileRead(path: Str) where ...;
            #   - Composite: effect IO includes: [FileRead, FileWrite, Console];
            #   - Stateful (multiline): effect HttpServer\n    states: [...]
            if re.match(r"^effect\s+\w+", stripped):
                # Collect the full effect definition (may span multiple lines)
                effect_lines = [stripped.rstrip(";").strip()]
                # Check if this is a multiline stateful effect (no semicolon,
                # no 'includes:', no params — next lines have states:/transition:)
                if not stripped.endswith(";") and "includes:" not in stripped:
                    k = i + 1
                    while k < len(lines) and k <= i + 20:
                        next_line = lines[k].strip()
                        if not next_line or next_line.startswith("//"):
                            k += 1
                            continue
                        # Stateful effect body lines: states:, initial:, transition <name>:
                        if re.match(
                            r"^(states|initial|transition)(\s+\w+)?\s*:", next_line,
                        ):
                            effect_lines.append(next_line.rstrip(";").strip())
                            k += 1
                            continue
                        break
                effects.append(" ".join(effect_lines))
                i += 1
                continue

            # Atom definitions
            atom_match = re.match(r"^(trusted\s+)?atom\s+\w+", stripped)
            if atom_match:
                signature = stripped.rstrip("{").strip()
                atom_entry: dict = {
                    "signature": signature,
                    "requires": "",
                    "ensures": "",
                    "effects": [],
                }
                # Scan the next lines for requires/ensures/effects
                j = i + 1
                while j < len(lines) and j <= i + 10:
                    next_stripped = lines[j].strip()
                    # Stop scanning at the next atom/type/struct/effect def
                    if re.match(
                        r"^(trusted\s+)?atom\s+\w+|^type\s+\w+|^struct\s+\w+|^effect\s+\w+",
                        next_stripped,
                    ):
                        break

                    req_match = re.match(r"requires\s*:\s*(.+?)\s*;", next_stripped)
                    if req_match:
                        atom_entry["requires"] = req_match.group(1).strip()
                        j += 1
                        continue

                    ens_match = re.match(r"ensures\s*:\s*(.+?)\s*;", next_stripped)
                    if ens_match:
                        atom_entry["ensures"] = ens_match.group(1).strip()
                        j += 1
                        continue

                    eff_match = re.match(r"effects\s*:\s*\[(.+?)\]", next_stripped)
                    if eff_match:
                        atom_entry["effects"] = [
                            e.strip() for e in eff_match.group(1).split(",")
                        ]
                        j += 1
                        continue

                    j += 1

                atoms.append(atom_entry)
                i += 1
                continue

            i += 1

        module_entry: dict = {
            "path": str(rel_path),
            "import": import_path,
            "description": description,
            "types": types,
            "atoms": atoms,
            "structs": structs,
            "effects": effects,
        }
        modules.append(module_entry)

    return json.dumps({"modules": modules}, indent=2, ensure_ascii=False)


# =============================================================
# analyze_std_gaps — Autonomous Proliferation (SI-5) の基盤
# =============================================================
# list_std_catalog が「今ある部品」を列挙するのに対し、
# analyze_std_gaps は「足りない部品」をヒューリスティクスで推論する。
# AI エージェントが次に実装すべき std コンポーネントを
# 自己判断するためのディスカバリ API。

# std/ ディレクトリの「欠落コンポーネント」を推論するためのルール。
# (condition_fn, proposal_factory) のタプルで、条件を満たすときに提案を生成する。
_STD_GAP_RULES: list[dict] = [
    {
        "target": "std/iter.mm",
        "reason": "コレクション走査の共通インターフェース。"
        "std/list.mm / std/alloc.mm の Vector / HashMap に反復子がない。",
        "depends_on": ["std/prelude.mm"],
        "difficulty": "medium",
        "trigger": {
            "has_container_without_iter": [
                "std/container",
                "std/list.mm",
                "std/alloc.mm",
            ],
            "missing": "std/iter.mm",
        },
    },
    {
        "target": "std/core.mm",
        "reason": "型変換の安全性証明が散在している。"
        "Size/Index/NonZero の公理と checked_add/sub/mul を一箇所に集約する。",
        "depends_on": ["std/prelude.mm"],
        "difficulty": "low",
        "trigger": {"missing": "std/core.mm"},
    },
    {
        "target": "std/trait/iterable.mm",
        "reason": "Vector/List/BoundedArray の共通インターフェース。"
        "Sequential トレイトを iterator と接続する。",
        "depends_on": ["std/prelude.mm", "std/alloc.mm"],
        "difficulty": "medium",
        "trigger": {
            "missing": "std/trait/iterable.mm",
            "requires_present": ["std/alloc.mm"],
        },
    },
    {
        "target": "std/hash.mm",
        "reason": "prelude.mm に Eq/Ord はあるが Hash の law が不完全。"
        "Hashable トレイトの具体実装と衝突耐性の law を提供する。",
        "depends_on": ["std/prelude.mm"],
        "difficulty": "medium",
        "trigger": {"missing": "std/hash.mm"},
    },
    {
        "target": "std/math/factorial.mm",
        "reason": "階乗計算。requires: n >= 0, ensures: result >= 1。"
        "Z3 整数理論で検証可能。",
        "depends_on": ["std/core.mm"],
        "difficulty": "medium",
        "trigger": {
            "missing": "std/math/factorial.mm",
            "requires_present": ["std/core.mm"],
        },
    },
    {
        "target": "std/container/sorted_map.mm",
        "reason": "ソート済みキーバリューマップ。"
        "挿入後のソート不変量を Z3 で検証。",
        "depends_on": ["std/container/bounded_array.mm"],
        "difficulty": "high",
        "trigger": {
            "missing": "std/container/sorted_map.mm",
            "requires_present": ["std/container/bounded_array.mm"],
        },
    },
    {
        "target": "std/string/validator.mm",
        "reason": "文字列バリデーション（is_numeric, is_alphanumeric 等）。"
        "RegTech デモの拡張に有用。",
        "depends_on": ["std/core.mm"],
        "difficulty": "low",
        "trigger": {"missing": "std/string/validator.mm"},
    },
    {
        "target": "std/math/fibonacci.mm",
        "reason": "フィボナッチ数列。"
        "loop invariant + decreases で停止性証明。",
        "depends_on": ["std/core.mm"],
        "difficulty": "medium",
        "trigger": {
            "missing": "std/math/fibonacci.mm",
            "requires_present": ["std/core.mm"],
        },
    },
]


# NOTE: _scan_std_imports and _collect_trusted_atoms live in std_graph_lib
# (imported at the top of this module). See ws5 for the extraction.


def _collect_todo_comments(std_dir: Path) -> list:
    """Scan std/ for TODO/FIXME/XXX/HACK markers and 'Phase X' roadmap notes."""
    results: list = []
    marker_re = re.compile(
        r"//.*?\b(TODO|FIXME|XXX|HACK|Phase\s+[A-Z0-9]+)\b[^\n]*",
        re.IGNORECASE,
    )
    for mm_file in sorted(std_dir.rglob("*.mm")):
        rel = str(mm_file.relative_to(std_dir.parent)).replace("\\", "/")
        try:
            lines = mm_file.read_text(encoding="utf-8").splitlines()
        except OSError:
            continue
        for idx, line in enumerate(lines):
            m = marker_re.search(line)
            if not m:
                continue
            results.append(
                {
                    "file": rel,
                    "line": idx + 1,
                    "text": line.strip().lstrip("/ ").strip(),
                },
            )
    return results


def _atom_name_index(std_dir: Path) -> list:
    """Return a sorted list of all atom names declared in std/.
    Used to compute usage frequency across tests/ and examples/.
    """
    names: set = set()
    atom_re = re.compile(r"^\s*(?:trusted\s+|async\s+)?atom\s+(\w+)")
    for mm_file in std_dir.rglob("*.mm"):
        try:
            for line in mm_file.read_text(encoding="utf-8").splitlines():
                m = atom_re.match(line)
                if m:
                    names.add(m.group(1))
        except OSError:
            continue
    return sorted(names)


def _count_usage(names: list, roots: list) -> dict:
    """For each atom name, count identifier-like occurrences across all .mm
    files under the given root directories. Skips the std/ directory itself.
    """
    counts: dict = {name: 0 for name in names}
    if not names:
        return counts
    patterns: dict = {name: re.compile(rf"\b{re.escape(name)}\b") for name in names}
    for root in roots:
        if not root.exists():
            continue
        for mm_file in root.rglob("*.mm"):
            try:
                text = mm_file.read_text(encoding="utf-8")
            except OSError:
                continue
            for name, pat in patterns.items():
                counts[name] += len(pat.findall(text))
    return {k: v for k, v in counts.items() if v > 0}


def _evaluate_rule(
    rule: dict,
    existing_paths: set,
    std_dir: Path,
) -> bool:
    """Return True if the rule's trigger conditions apply (i.e., the
    suggested component looks like a real gap worth proposing)."""
    trigger = rule.get("trigger", {})
    missing = trigger.get("missing")
    if missing and missing in existing_paths:
        return False
    for required in trigger.get("requires_present", []):
        if required not in existing_paths:
            return False
    container_check = trigger.get("has_container_without_iter")
    if container_check:
        has_container = any(
            (std_dir.parent / path).exists()
            or (path.endswith("/") and (std_dir.parent / path.rstrip("/")).exists())
            for path in container_check
        )
        if not has_container:
            return False
    return True


# ---------------------------------------------------------------------------
# Weighted scoring (SI-5 Phase 1-C)
# ---------------------------------------------------------------------------
# Higher scores = higher priority. Scoring balances four axes:
#   * usage_demand: how often atoms from depended-on modules are referenced
#     in tests/examples (heavy users of a module should be extended first)
#   * dep_depth:    how many other std modules depend on the target area
#     (lower-layer modules benefit more modules when improved)
#   * trusted_density: how many trusted atoms (proof holes) live in the
#     target area (more proof holes = higher priority to shore up)
#   * difficulty_penalty: subtract a small amount for higher difficulty


def _compute_weighted_score(
    proposal: dict,
    usage_frequency: dict,
    dependency_graph: dict,
    trusted_atoms: list,
) -> float:
    """Compute a weighted priority score for a proposal in [~-0.09, ~0.85]."""

    # --- Axis 1: Usage demand --------------------------------------------------
    deps = proposal.get("depends_on", []) or []
    if deps:
        # Atoms declared in dependency files are treated as proxies for
        # "how heavily used is this area of std". We sum the usage frequency
        # across all atoms declared in the dependency modules and normalize
        # by the number of deps to get an average.
        per_dep_demand = []
        for dep in deps:
            # usage_frequency is keyed by atom name, not module path, so
            # approximate by summing frequency of atoms defined in `dep` —
            # but we don't have that mapping readily. Instead, approximate
            # by checking if any atom name substring matches the dep stem
            # (common heuristic: std/list.mm -> list usage).
            stem = dep.rsplit("/", 1)[-1].removesuffix(".mm")
            per_dep_demand.append(
                sum(v for k, v in usage_frequency.items() if stem and stem in k.lower()),
            )
        usage_demand = sum(per_dep_demand) / max(1, len(deps))
    else:
        usage_demand = 0.0

    # --- Axis 2: Dependency depth ---------------------------------------------
    # Count how many std files import any of the dep modules the proposal
    # builds on. Higher = deeper in the dependency stack = bigger blast
    # radius for improvements.
    dep_depth = 0
    for _file, imports in dependency_graph.items():
        if any(d in imports for d in deps):
            dep_depth += 1

    # --- Axis 3: Trusted density ---------------------------------------------
    # Count trusted atoms living in the proposal's "target area" — the
    # directory of `name`. If the target has no directory component, fall
    # back to counting trusted atoms in any dependency file area.
    name = proposal.get("name", "")
    target_area = ""
    if "/" in name:
        target_area = name.rsplit("/", 1)[0]

    if target_area:
        trusted_in_area = sum(
            1 for t in trusted_atoms if t.get("file", "").startswith(target_area)
        )
    else:
        trusted_in_area = 0
    # Also account for trusted atoms in the immediate deps (shoring up
    # holes in the foundations the proposal builds on).  Skip deps already
    # covered by the target_area prefix pass to avoid double-counting.
    for dep in deps:
        if target_area and dep.startswith(target_area):
            continue  # already counted in the target_area pass above
        trusted_in_area += sum(
            1 for t in trusted_atoms if t.get("file", "") == dep
        )

    # --- Axis 4: Difficulty penalty -------------------------------------------
    diff_penalty = {"low": 0.0, "medium": 0.3, "high": 0.6}.get(
        proposal.get("difficulty", ""),
        0.5,
    )

    # Weighted combination (positive weights sum to 0.85; all four sum to
    # 1.0 including the penalty weight). Each axis is saturated at a
    # reasonable ceiling before combining so a single runaway axis cannot
    # dominate the score.  Effective range is [~-0.09, ~0.85].
    w_usage, w_depth, w_trusted, w_diff = 0.30, 0.30, 0.25, 0.15
    usage_norm = min(usage_demand / 10.0, 1.0)
    depth_norm = min(dep_depth / 5.0, 1.0)
    trusted_norm = min(trusted_in_area / 3.0, 1.0)

    return (
        w_usage * usage_norm
        + w_depth * depth_norm
        + w_trusted * trusted_norm
        - w_diff * diff_penalty
    )


# ---------------------------------------------------------------------------
# Dependency graph rendering (SI-5 Phase 1-C)
# ---------------------------------------------------------------------------
# Converts the output of _scan_std_imports() + _collect_trusted_atoms()
# into Mermaid or Graphviz-DOT source that can be rendered on GitHub or
# piped through `dot`.


# NOTE: _sanitize_node_id, _count_atoms_per_file, _classify_health,
# _render_std_graph_{mermaid,dot}, and _trusted_by_file_counts live in
# std_graph_lib (imported at the top of this module). See ws5 for the
# extraction.


@mcp.tool()
def visualize_std_graph(format: str = "mermaid") -> str:
    """Visualize the std/ dependency graph in Mermaid or DOT format.

    Args:
        format: "mermaid" (default) or "dot".

    Returns:
        Graph source text that can be rendered by Mermaid or Graphviz.
        Nodes are colored by verification health:
        - Green  (rounded)   — fully verified, no trusted atoms
        - Yellow (hexagon)   — verified but has trusted atoms (proof holes)
        - Red    (double)    — verification failed or missing

    Node labels include "<path>\\n<N> atoms, <M> trusted" so the density of
    proof holes is immediately visible. Intended to be consumed by SI-5
    Phase 1-C visualizers and by AI agents planning std/ work.
    """
    repo_root = Path(__file__).parent.absolute()
    std_dir = repo_root / "std"
    if not std_dir.exists():
        return json.dumps({"error": "std/ directory not found"})

    fmt = (format or "mermaid").lower().strip()
    if fmt not in {"mermaid", "dot"}:
        return json.dumps(
            {"error": f"unsupported format '{format}'. Use 'mermaid' or 'dot'."},
        )

    dependency_graph = _scan_std_imports(std_dir)
    trusted_atoms = _collect_trusted_atoms(std_dir)
    trusted_by_file = _trusted_by_file_counts(trusted_atoms)
    atoms_by_file = _count_atoms_per_file(std_dir)
    # No in-process verification is performed here; "red" is reserved for
    # files that a downstream CI step has recorded as failing. Callers can
    # enrich the graph via the visualizer/ generator.
    failed_files: set = set()

    if fmt == "mermaid":
        return _render_std_graph_mermaid(
            dependency_graph,
            trusted_by_file,
            atoms_by_file,
            failed_files,
        )
    return _render_std_graph_dot(
        dependency_graph,
        trusted_by_file,
        atoms_by_file,
        failed_files,
    )


@mcp.tool()
def analyze_std_gaps() -> str:
    """Analyze the mumei std/ library for missing components and propose
    next implementation targets. AI agents should call this before starting
    autonomous std expansion work — it combines dependency graph analysis,
    trusted-atom (proof hole) detection, TODO comment scanning, and usage
    frequency across tests/ and examples/ to produce a ranked list of
    proposed new std modules.

    Returns JSON with: dependency_graph, trusted_atoms, todo_comments,
    usage_frequency, and proposals (top 3 ranked by priority).

    Each proposal carries a `score` field (higher = higher priority) computed
    from four axes: usage demand, dependency depth, trusted density, and a
    difficulty penalty (SI-5 Phase 1-C weighted scoring).
    """
    repo_root = Path(__file__).parent.absolute()
    std_dir = repo_root / "std"
    if not std_dir.exists():
        return json.dumps({"error": "std/ directory not found"})

    dependency_graph = _scan_std_imports(std_dir)
    trusted_atoms = _collect_trusted_atoms(std_dir)
    todo_comments = _collect_todo_comments(std_dir)

    atom_names = _atom_name_index(std_dir)
    usage_roots = [repo_root / "tests", repo_root / "examples"]
    usage_frequency = _count_usage(atom_names, usage_roots)

    existing_paths = set(dependency_graph.keys())

    proposals: list = []
    for rule in _STD_GAP_RULES:
        if not _evaluate_rule(rule, existing_paths, std_dir):
            continue
        proposals.append(
            {
                "name": rule["target"],
                "reason": rule["reason"],
                "depends_on": rule["depends_on"],
                "difficulty": rule["difficulty"],
            },
        )

    # SI-5 Phase 1-C: 3-axis weighted scoring. Higher score wins.
    for p in proposals:
        p["score"] = round(
            _compute_weighted_score(
                p,
                usage_frequency,
                dependency_graph,
                trusted_atoms,
            ),
            4,
        )

    # Stable tie-break: higher score first, then fewer unmet deps, then
    # lower difficulty. Note: the old sort key was (difficulty, unmet);
    # the new key intentionally prioritises unmet deps over difficulty.
    difficulty_weight = {"low": 0, "medium": 1, "high": 2}

    def _rank_key(p: dict) -> tuple:
        unmet = sum(
            1 for dep in p["depends_on"] if dep not in existing_paths
        )
        diff = difficulty_weight.get(p["difficulty"], 3)
        # Negate score so higher scores sort first under ascending sort.
        return (-p["score"], unmet, diff)

    proposals.sort(key=_rank_key)
    for i, p in enumerate(proposals[:3], start=1):
        p["priority"] = i
    proposals = proposals[:3]

    # Stable order for JSON output.
    usage_frequency_sorted = dict(
        sorted(usage_frequency.items(), key=lambda kv: (-kv[1], kv[0])),
    )

    result = {
        "dependency_graph": dependency_graph,
        "trusted_atoms": trusted_atoms,
        "todo_comments": todo_comments,
        "usage_frequency": usage_frequency_sorted,
        "proposals": proposals,
    }
    return json.dumps(result, indent=2, ensure_ascii=False)


# ---------------------------------------------------------------------------
# P10: cross-repo MCP tools
# ---------------------------------------------------------------------------
#
# These tools mirror utilities that previously only existed in
# mumei-agent (``agent/std_health.py``) or required a local mumei
# checkout, so external MCP clients (Claude Code / Devin / Codex) can
# now query proof health, retrieve proof certificates, and generate
# documentation through the same FastMCP transport as the rest of the
# Mumei-Forge tools.


def _resolve_mumei_invocation() -> list[str]:
    """Pick how to invoke ``mumei``.

    Honors ``MUMEI_BIN`` (which may include arguments such as
    ``cargo run --manifest-path … --``), then falls back to a plain
    ``mumei`` on ``$PATH``, then to ``cargo run --`` from the repo
    root.
    """
    import os
    import shlex

    env = os.environ.get("MUMEI_BIN", "").strip()
    if env:
        parts = shlex.split(env)
        if parts and (shutil.which(parts[0]) or Path(parts[0]).exists()):
            return parts
    if shutil.which("mumei"):
        return ["mumei"]
    return ["cargo", "run", "--quiet", "--"]


def _parse_atoms(text: str) -> list[str]:
    """Return atom names defined or trusted in *text* (best effort)."""
    names: list[str] = []
    for line in text.splitlines():
        m = re.match(r"^\s*(?:trusted\s+|async\s+)?atom\s+([A-Za-z_][A-Za-z0-9_]*)", line)
        if m:
            names.append(m.group(1))
    return names


def _parse_trusted_atoms(text: str) -> list[str]:
    names: list[str] = []
    for line in text.splitlines():
        m = re.match(r"^\s*trusted\s+atom\s+([A-Za-z_][A-Za-z0-9_]*)", line)
        if m:
            names.append(m.group(1))
    return names


_TODO_RE = re.compile(
    r"//.*?\b(TODO|FIXME|XXX|HACK)\b",
    re.IGNORECASE,
)


def _count_todos(text: str) -> int:
    return sum(1 for line in text.splitlines() if _TODO_RE.search(line))


def _compute_health_score(
    total_atoms: int,
    verified_atoms: int,
    trusted_atoms: int,
    todo_count: int,
) -> float:
    """Mirror of ``agent.std_health.compute_health_score``.

    score = (verified - trusted) / total - 0.01 * todo_count, clamped
    to ``[0.0, 1.0]``.
    """
    if total_atoms <= 0:
        return 0.0
    base = (verified_atoms - trusted_atoms) / total_atoms
    penalty = 0.01 * todo_count
    score = base - penalty
    if score < 0.0:
        return 0.0
    if score > 1.0:
        return 1.0
    return round(score, 4)


@mcp.tool()
def measure_std_health() -> str:
    """Measure proof-health metrics for the std/ library.

    Mirrors :func:`agent.std_health.measure_health` so external MCP
    clients can monitor std/ health without installing mumei-agent.

    Returns JSON with ``total_files``, ``verified_files``,
    ``failed_files``, ``total_atoms``, ``verified_atoms``,
    ``trusted_atoms``, ``health_score``, ``todo_count``, and
    per-file ``details``.
    """
    root_dir = Path(__file__).parent.absolute()
    std_dir = root_dir / "std"
    if not std_dir.exists():
        return json.dumps({"error": "std/ directory not found"})

    cmd_prefix = _resolve_mumei_invocation()

    total_files = 0
    verified_files = 0
    failed_files = 0
    verify_unavailable_files = 0
    total_atoms = 0
    verified_atoms = 0
    trusted_atoms = 0
    todo_count = 0
    details: list[dict] = []

    for mm_file in sorted(std_dir.rglob("*.mm")):
        # Skip files under std/certs/ (proof outputs, not source).
        try:
            rel_to_std = mm_file.relative_to(std_dir)
        except ValueError:
            continue
        if rel_to_std.parts and rel_to_std.parts[0] == "certs":
            continue

        rel = mm_file.relative_to(root_dir).as_posix()
        try:
            text = mm_file.read_text(encoding="utf-8")
        except OSError as exc:
            details.append(
                {"file": rel, "status": "io_error", "error": str(exc)}
            )
            continue

        atoms = _parse_atoms(text)
        trusted = _parse_trusted_atoms(text)
        file_total = len(atoms)
        file_trusted = len(trusted)
        file_todo = _count_todos(text)

        total_files += 1
        total_atoms += file_total
        todo_count += file_todo

        try:
            result = subprocess.run(
                [*cmd_prefix, "verify", "--json", str(mm_file)],
                cwd=root_dir,
                capture_output=True,
                text=True,
                timeout=120,
            )
        except (FileNotFoundError, subprocess.TimeoutExpired) as exc:
            verify_unavailable_files += 1
            details.append(
                {
                    "file": rel,
                    "status": "verify_unavailable",
                    "error": str(exc),
                    "atoms": file_total,
                    "trusted_atoms": file_trusted,
                    "todo": file_todo,
                }
            )
            continue

        success = result.returncode == 0
        if success:
            verified_files += 1
            verified_atoms += file_total
            trusted_atoms += file_trusted
            status = "verified"
        else:
            failed_files += 1
            status = "failed"

        details.append(
            {
                "file": rel,
                "status": status,
                "atoms": file_total,
                "trusted_atoms": file_trusted,
                "todo": file_todo,
            }
        )

    health_score = _compute_health_score(
        total_atoms, verified_atoms, trusted_atoms, todo_count
    )
    payload = {
        "total_files": total_files,
        "verified_files": verified_files,
        "failed_files": failed_files,
        "verify_unavailable_files": verify_unavailable_files,
        "total_atoms": total_atoms,
        "verified_atoms": verified_atoms,
        "trusted_atoms": trusted_atoms,
        "todo_count": todo_count,
        "health_score": health_score,
        "details": details,
    }
    return json.dumps(payload, indent=2, ensure_ascii=False)


@mcp.tool()
def get_proof_certificate(module_path: str) -> str:
    """Return the proof certificate JSON for *module_path*.

    *module_path* is a repo-relative module path such as ``std/core.mm``
    or ``std/core``.  The tool first looks under ``std/certs/`` for a
    matching ``<module>.proof.json``, then falls back to the bundled
    ``std-proof-bundle.json`` if present.

    Returns a JSON object with either the certificate payload or an
    ``error`` field describing why no certificate is available.
    """
    root_dir = Path(__file__).parent.absolute()
    if not module_path or not isinstance(module_path, str):
        return json.dumps({"error": "module_path is required"})

    # Normalise the module path: strip leading "std/", trailing ".mm",
    # collapse backslashes.  We do not allow absolute paths or "..".
    cleaned = module_path.replace("\\", "/").strip()
    if cleaned.startswith("./"):
        cleaned = cleaned[2:]
    if ".." in Path(cleaned).parts or Path(cleaned).is_absolute():
        return json.dumps({"error": "module_path must be repo-relative"})

    # Drop the leading 'std/' prefix when present so we can compose
    # paths under ``std/certs/`` cleanly.
    rel = cleaned[4:] if cleaned.startswith("std/") else cleaned
    if rel.endswith(".mm"):
        rel = rel[: -len(".mm")]

    certs_dir = root_dir / "std" / "certs"
    candidate = certs_dir / f"{rel}.proof.json"
    key_with_prefix = f"std/{rel}"
    if candidate.exists():
        try:
            cert = json.loads(candidate.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            return json.dumps({"error": f"failed to read certificate: {exc}"})
        return json.dumps(
            {
                "module": key_with_prefix,
                "source": "std/certs",
                "certificate": cert,
            },
            indent=2,
            ensure_ascii=False,
        )

    bundle = root_dir / "std-proof-bundle.json"
    if bundle.exists():
        try:
            data = json.loads(bundle.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            return json.dumps(
                {"error": f"failed to read std-proof-bundle.json: {exc}"}
            )
        modules = data.get("modules") or {}
        cert = modules.get(key_with_prefix) or modules.get(rel)
        if cert is not None:
            return json.dumps(
                {
                    "module": key_with_prefix,
                    "source": "std-proof-bundle.json",
                    "certificate": cert,
                    "bundle_version": data.get("bundle_version"),
                    "mumei_version": data.get("mumei_version"),
                },
                indent=2,
                ensure_ascii=False,
            )

    return json.dumps(
        {
            "error": "no proof certificate found",
            "module": cleaned,
            "looked_at": [str(candidate.relative_to(root_dir)), "std-proof-bundle.json"],
        }
    )


@mcp.tool()
def generate_doc(source_code: str, format: str = "json") -> str:
    """Run ``mumei doc`` on *source_code* and return structured docs.

    The mumei compiler's ``doc`` subcommand supports
    ``--format json|markdown|html`` and writes its output under an
    ``--output`` directory.  We point the output at a NamedTemporaryFile
    directory so the call does not pollute the repo.

    Args:
        source_code: ``.mm`` source code to document.
        format: One of ``json`` (default), ``markdown``, ``html``.  For
            ``json`` we return the parsed object directly so MCP
            clients don't have to re-parse the inner payload.
    """
    if not isinstance(source_code, str) or not source_code.strip():
        return json.dumps({"error": "source_code must be non-empty"})
    fmt = (format or "json").strip().lower()
    if fmt not in ("json", "markdown", "html"):
        return json.dumps({"error": f"unsupported format: {format!r}"})

    root_dir = Path(__file__).parent.absolute()
    cmd_prefix = _resolve_mumei_invocation()
    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = Path(tmpdir)
        source_path = tmp_path / "input.mm"
        source_path.write_text(source_code, encoding="utf-8")

        try:
            result = subprocess.run(
                [
                    *cmd_prefix,
                    "doc",
                    str(source_path),
                    "--output",
                    str(tmp_path / "out"),
                    "--format",
                    fmt,
                ],
                cwd=root_dir,
                capture_output=True,
                text=True,
                timeout=120,
            )
        except (FileNotFoundError, subprocess.TimeoutExpired) as exc:
            return json.dumps(
                {"error": f"mumei doc unavailable: {exc}"}
            )

        if result.returncode != 0:
            return json.dumps(
                {
                    "error": "mumei doc failed",
                    "stderr": result.stderr,
                    "stdout": result.stdout,
                }
            )

        if fmt == "json":
            text = result.stdout.strip()
            if text:
                try:
                    parsed = json.loads(text)
                    return json.dumps(
                        {"format": "json", "doc": parsed},
                        indent=2,
                        ensure_ascii=False,
                    )
                except json.JSONDecodeError:
                    pass

            # Some doc backends write to <out>/doc.json instead of
            # stdout.  Walk the output dir and surface whatever we find.
            out_dir = tmp_path / "out"
            collected: dict[str, object] = {}
            if out_dir.exists():
                for f in sorted(out_dir.rglob("*.json")):
                    try:
                        collected[
                            str(f.relative_to(out_dir).as_posix())
                        ] = json.loads(f.read_text(encoding="utf-8"))
                    except (OSError, json.JSONDecodeError):
                        continue
            if collected:
                return json.dumps(
                    {"format": "json", "doc": collected},
                    indent=2,
                    ensure_ascii=False,
                )
            return json.dumps(
                {"error": "mumei doc produced no JSON output", "stdout": text}
            )

        # markdown / html: return raw text (and any rendered files).
        out_dir = tmp_path / "out"
        files: dict[str, str] = {}
        if out_dir.exists():
            for f in sorted(out_dir.rglob("*")):
                if f.is_file():
                    try:
                        files[str(f.relative_to(out_dir).as_posix())] = (
                            f.read_text(encoding="utf-8")
                        )
                    except (OSError, UnicodeDecodeError):
                        continue
        return json.dumps(
            {
                "format": fmt,
                "stdout": result.stdout,
                "files": files,
            },
            ensure_ascii=False,
        )


if __name__ == "__main__":
    mcp.run()
