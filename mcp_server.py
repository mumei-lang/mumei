import os
import re
import shutil
import subprocess
import json
import tempfile
from pathlib import Path
from mcp.server.fastmcp import FastMCP
from dotenv import load_dotenv

# Load environment variables
load_dotenv()

# Initialize MCP server
mcp = FastMCP("Mumei-Forge")

# Module-level session state for effect boundary overrides
_session_effects: dict = {
    "allowed": [],
    "denied": [],
    "source": "default",  # "default" | "mumei.toml" | "session_override"
}

# Visualizer sync config
# true: also copy report.json to visualizer/ (for Streamlit dashboard)
# false: MCP response only (default)
VISUALIZER_SYNC = os.getenv("ENABLE_VISUALIZER_SYNC", "false").lower() == "true"
HISTORY_FILE = Path(__file__).parent.absolute() / "visualizer" / "report_history.json"


def _sync_to_visualizer(report_file: Path, root_dir: Path) -> None:
    """Copy report.json to visualizer/ and append to history.

    NOTE: Nearly identical logic exists in self_healing.py (sync_to_visualizer).
    If you change this, update self_healing.py as well (or extract into a shared
    module in the future).
    """
    if not VISUALIZER_SYNC:
        return
    if not report_file.exists():
        return

    vis_dir = root_dir / "visualizer"
    vis_dir.mkdir(exist_ok=True)
    shutil.copy(report_file, vis_dir / "report.json")

    # Append to history (with file lock to prevent corruption when
    # mcp_server.py and self_healing.py run concurrently)
    import datetime
    import fcntl

    entry = json.loads(report_file.read_text(encoding="utf-8"))
    entry["timestamp"] = datetime.datetime.now(datetime.timezone.utc).isoformat()

    lock_file = HISTORY_FILE.parent / ".report_history.lock"
    lock_file.parent.mkdir(exist_ok=True)
    with open(lock_file, "w") as lf:
        fcntl.flock(lf, fcntl.LOCK_EX)
        try:
            history = []
            if HISTORY_FILE.exists():
                try:
                    history = json.loads(HISTORY_FILE.read_text(encoding="utf-8"))
                except (json.JSONDecodeError, OSError):
                    history = []
            history.append(entry)
            HISTORY_FILE.write_text(
                json.dumps(history, indent=2, ensure_ascii=False), encoding="utf-8"
            )
        finally:
            fcntl.flock(lf, fcntl.LOCK_UN)

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
def forge_blade(source_code: str, output_name: str = "katana") -> str:
    """
    Verify Mumei code and generate Rust/Go/TS output.
    The build command writes report.json to the -o directory, so reports are isolated per request.
    """
    root_dir = Path(__file__).parent.absolute()

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
            text=True
        )

        response_parts = []

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
            try:
                _sync_to_visualizer(report_file, root_dir)
            except Exception:
                pass
        else:
            response_parts.append(
                '### Semantic Feedback\n'
                '```json\n{"status": "no_report_available"}\n```'
            )

        if result.returncode == 0:
            response_parts.insert(0, f"Forge succeeded: '{output_name}'")
            # Collect generated artifacts
            for ext in [".rs", ".go", ".ts", ".ll"]:
                gen_file = tmp_path / f"{output_name}{ext}"
                if gen_file.exists():
                    # Set syntax highlighting based on extension
                    lang = "rust" if ext in [".rs", ".ll"] else "go" if ext == ".go" else "typescript"
                    content = gen_file.read_text(encoding="utf-8")
                    response_parts.append(f"\n### Generated: {output_name}{ext}\n```{lang}\n{content}\n```")

            return "\n".join(response_parts)
        else:
            # On failure: return evidence (report) and error log together
            response_parts.insert(0, f"Forge failed: logical flaw detected.")
            if result.stderr:
                response_parts.append(f"\n### Error Details\n{result.stderr}")

            return "\n".join(response_parts)

@mcp.tool()
def self_heal_loop() -> str:
    """
    Run self_healing.py to start an AI-driven autonomous fix loop (targeting sword_test.mm).
    """
    root_dir = Path(__file__).parent.absolute()

    try:
        result = subprocess.run(
            ["python", "self_healing.py"],
            cwd=root_dir,
            capture_output=True,
            text=True,
            timeout=300
        )
        if result.returncode == 0:
            return f"Self-healing completed:\n{result.stdout}"
        else:
            return f"Self-healing failed:\n{result.stderr}\n{result.stdout}"
    except subprocess.TimeoutExpired:
        return "Error: Self-healing loop timed out (300s)."
    except Exception as e:
        return f"Execution error: {str(e)}"

@mcp.tool()
def validate_logic(source_code: str) -> str:
    """
    Run formal verification (Z3) only on Mumei code.
    No code generation — returns verification results and counter-examples.
    Used as the verification step when AI iteratively fixes .mm code.

    Uses --report-dir to write report.json directly into a per-request temp
    directory, making concurrent calls safe.
    """
    root_dir = Path(__file__).parent.absolute()

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
            text=True
        )

        response_parts = []

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
            try:
                _sync_to_visualizer(report_file, root_dir)
            except Exception:
                pass
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
            try:
                _sync_to_visualizer(report_file, root_dir)
            except Exception:
                pass
        else:
            response_parts.append(
                '### Semantic Feedback\n'
                '```json\n{"status": "no_report_available"}\n```'
            )

        if result.returncode == 0:
            response_parts.insert(0, f"{command} succeeded: '{output_name}'")
            # Collect generated artifacts
            for ext in [".rs", ".go", ".ts", ".ll"]:
                gen_file = tmp_path / f"{output_name}{ext}"
                if gen_file.exists():
                    lang = (
                        "rust" if ext in [".rs", ".ll"]
                        else "go" if ext == ".go"
                        else "typescript"
                    )
                    content = gen_file.read_text(encoding="utf-8")
                    response_parts.append(
                        f"\n### Generated: {output_name}{ext}"
                        f"\n```{lang}\n{content}\n```"
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
def self_heal_with_effects(
    source_code: str,
    allowed_effects: "list[str] | None" = None,
    max_attempts: int = 5,
) -> str:
    """
    Run an effect-aware self-healing loop on the given source code.

    1. Validates the code with Z3 (including effect verification)
    2. If effect violations are found, generates fixes using logical resolution paths
    3. Re-validates until the code passes or max_attempts is reached

    The allowed_effects parameter restricts which effects the healed code may use.
    If empty or None, no effect restrictions are applied.
    """
    if allowed_effects is None:
        allowed_effects = []

    results = []
    current_code = source_code

    # Temporarily set session effects if provided
    if allowed_effects:
        set_allowed_effects(allowed=allowed_effects)
        results.append(
            f"### Effect Boundary: {allowed_effects}\n"
        )

    root_dir = Path(__file__).parent.absolute()

    try:
        for attempt in range(max_attempts):
            # Run validation — this writes report.json to root_dir (compiler cwd)
            # then validate_logic moves it into a temp directory.  We run the
            # compiler directly here so we can read report.json before it is moved.
            with tempfile.TemporaryDirectory() as tmpdir:
                tmp_path = Path(tmpdir)
                source_path = tmp_path / "input.mm"
                source_path.write_text(current_code, encoding="utf-8")

                compile_result = subprocess.run(
                    ["cargo", "run", "--", "verify",
                     "--report-dir", str(tmp_path),
                     str(source_path)],
                    cwd=root_dir,
                    capture_output=True,
                    text=True,
                )

                if compile_result.returncode == 0:
                    results.append(f"### Attempt {attempt + 1}: PASSED")
                    results.append("Verification passed: no logical flaws detected.")
                    results.append(
                        f"\n### Final Code\n```mumei\n{current_code}\n```"
                    )
                    return "\n".join(results)

                results.append(f"### Attempt {attempt + 1}: FAILED")
                error_log = compile_result.stderr or compile_result.stdout or ""
                results.append(f"```\n{error_log}\n```")

                # Read report.json from per-request temp directory (concurrent-safe)
                report_file = tmp_path / "report.json"
                report_data = {}
                if report_file.exists():
                    try:
                        report_data = json.loads(
                            report_file.read_text(encoding="utf-8")
                        )
                    except (json.JSONDecodeError, OSError):
                        pass

                # Generate fix suggestion based on violation type
                violation_type = report_data.get("violation_type", "")
                ev = report_data.get("effect_violation", {})

                if violation_type == "effect_mismatch":
                    fix_hint = (
                        f"Effect mismatch: '{ev.get('source_operation', '')}' "
                        f"requires [{ev.get('required_effect', '')}]. "
                        f"Resolution: {ev.get('resolution_paths', [{}])[0].get('description', '')}"
                    )
                    results.append(f"\n**Fix hint**: {fix_hint}")
                elif violation_type == "effect_propagation":
                    fix_hint = (
                        f"Propagation: '{ev.get('caller', '')}' missing "
                        f"{ev.get('missing_effects', [])}. "
                        f"Resolution: {ev.get('resolution_paths', [{}])[0].get('description', '')}"
                    )
                    results.append(f"\n**Fix hint**: {fix_hint}")

                # Use self_healing.get_fix_from_ai to generate a fix and update current_code
                try:
                    from self_healing import get_fix_from_ai

                    fixed_code = get_fix_from_ai(current_code, error_log, report_data)
                    if fixed_code and fixed_code != current_code:
                        current_code = fixed_code
                        results.append("AI-generated fix applied. Retrying...\n")
                    else:
                        results.append("AI could not generate a different fix.\n")
                        break
                except Exception as exc:
                    results.append(f"AI fix generation failed: {exc}\n")
                    break

        results.append("Self-healing exhausted max attempts.")
        return "\n".join(results)
    finally:
        # Always reset session effects, even on exception
        if allowed_effects:
            set_allowed_effects()


if __name__ == "__main__":
    mcp.run()
