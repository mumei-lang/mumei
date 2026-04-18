import re
import shutil
import subprocess
import json
import tempfile
from pathlib import Path
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("Mumei-Forge")

# Module-level session state for effect boundary overrides
_session_effects: dict = {
    "allowed": [],
    "denied": [],
    "source": "default",  # "default" | "mumei.toml" | "session_override"
}

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
    Verify Mumei code and generate LLVM IR output.
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
]


def _scan_std_imports(std_dir: Path) -> dict:
    """Scan all .mm files under std/ and build a dependency graph.
    Returns a dict mapping each std/*.mm file path to a sorted list of
    import targets (e.g., "std/prelude.mm"). Files with no imports map to [].
    Only imports resolving to std/ modules are included.
    """
    if not std_dir.exists():
        return {}

    available: dict = {}
    for mm_file in std_dir.rglob("*.mm"):
        rel = str(mm_file.relative_to(std_dir.parent)).replace("\\", "/")
        import_path = rel[: -len(".mm")]
        available[import_path] = rel

    dependency_graph: dict = {}
    # Accept both `import "std/xxx" as alias;` and `import "std/xxx";`
    import_re = re.compile(r'^\s*import\s+"([^"]+)"\s*(?:as\s+\w+\s*)?;')
    for mm_file in sorted(std_dir.rglob("*.mm")):
        rel = str(mm_file.relative_to(std_dir.parent)).replace("\\", "/")
        try:
            text = mm_file.read_text(encoding="utf-8")
        except OSError:
            dependency_graph[rel] = []
            continue
        deps: list = []
        for line in text.splitlines():
            m = import_re.match(line)
            if not m:
                continue
            target = m.group(1).strip()
            resolved = available.get(target)
            if resolved and resolved != rel and resolved not in deps:
                deps.append(resolved)
        dependency_graph[rel] = sorted(deps)
    return dependency_graph


def _collect_trusted_atoms(std_dir: Path) -> list:
    """Scan for `trusted atom <name>` and return one entry per occurrence.
    Each entry contains file, atom, line number, and a short reason derived
    from the surrounding comments or body if available.
    """
    results: list = []
    trusted_re = re.compile(r"^\s*trusted\s+atom\s+(\w+)")
    for mm_file in sorted(std_dir.rglob("*.mm")):
        rel = str(mm_file.relative_to(std_dir.parent)).replace("\\", "/")
        try:
            lines = mm_file.read_text(encoding="utf-8").splitlines()
        except OSError:
            continue
        for idx, line in enumerate(lines):
            m = trusted_re.match(line)
            if not m:
                continue
            atom_name = m.group(1)
            # Heuristic reason extraction: grab the nearest preceding
            # comment block (// ...) or note a stub/empty body.
            reason = ""
            look = idx - 1
            while look >= 0 and lines[look].strip().startswith("//"):
                reason = lines[look].strip().lstrip("/ ").strip()
                look -= 1
            if not reason:
                # Peek up to 6 lines ahead for `body:` contents to detect stubs.
                end = min(idx + 10, len(lines))
                body_text = " ".join(l.strip() for l in lines[idx + 1 : end])
                if re.search(r"body\s*:\s*\{\s*\}", body_text):
                    reason = "body is stub"
                else:
                    reason = "trusted (proof hole)"
            results.append(
                {
                    "file": rel,
                    "atom": atom_name,
                    "line": idx + 1,
                    "reason": reason,
                },
            )
    return results


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


def _sanitize_node_id(path: str) -> str:
    """Return a Mermaid/DOT-safe identifier for a std file path."""
    return re.sub(r"[^A-Za-z0-9_]", "_", path)


def _count_atoms_per_file(std_dir: Path) -> dict:
    """Return {rel_path: total_atom_count} for every .mm file in std/."""
    atom_re = re.compile(r"^\s*(?:trusted\s+|async\s+)?atom\s+\w+")
    counts: dict = {}
    for mm_file in std_dir.rglob("*.mm"):
        rel = str(mm_file.relative_to(std_dir.parent)).replace("\\", "/")
        try:
            text = mm_file.read_text(encoding="utf-8")
        except OSError:
            counts[rel] = 0
            continue
        counts[rel] = sum(1 for line in text.splitlines() if atom_re.match(line))
    return counts


def _classify_health(
    rel_path: str,
    trusted_by_file: dict,
    failed_files: set,
) -> str:
    """Return one of "green", "yellow", "red" for a std file.

    * "red"    — verification has failed or the file is explicitly marked broken.
    * "yellow" — at least one trusted atom (proof hole) present.
    * "green"  — fully verified, no trusted atoms.
    """
    if rel_path in failed_files:
        return "red"
    if trusted_by_file.get(rel_path, 0) > 0:
        return "yellow"
    return "green"


def _render_std_graph_mermaid(
    dependency_graph: dict,
    trusted_by_file: dict,
    atoms_by_file: dict,
    failed_files: set,
) -> str:
    """Render the std/ dependency graph as Mermaid `graph TD` source."""
    lines = ["graph TD"]
    # Node declarations: shape carries health semantics.
    #   green  -> rounded rectangle  id(label)
    #   yellow -> hexagon            id{{label}}
    #   red    -> doubled border     id[[label]]
    for path in sorted(dependency_graph.keys()):
        node_id = _sanitize_node_id(path)
        atoms = atoms_by_file.get(path, 0)
        trusted = trusted_by_file.get(path, 0)
        label = f"{path}\\n{atoms} atoms, {trusted} trusted"
        health = _classify_health(path, trusted_by_file, failed_files)
        if health == "green":
            lines.append(f'    {node_id}("{label}")')
        elif health == "yellow":
            lines.append(f'    {node_id}{{{{"{label}"}}}}')
        else:  # red
            lines.append(f'    {node_id}[["{label}"]]')

    # Edges.
    for path in sorted(dependency_graph.keys()):
        for dep in dependency_graph[path]:
            src = _sanitize_node_id(path)
            dst = _sanitize_node_id(dep)
            lines.append(f"    {src} --> {dst}")

    # Color classes.
    lines.append("    classDef green fill:#d4edda,stroke:#28a745,color:#155724;")
    lines.append("    classDef yellow fill:#fff3cd,stroke:#ffc107,color:#856404;")
    lines.append("    classDef red fill:#f8d7da,stroke:#dc3545,color:#721c24;")

    green_nodes, yellow_nodes, red_nodes = [], [], []
    for path in sorted(dependency_graph.keys()):
        node_id = _sanitize_node_id(path)
        health = _classify_health(path, trusted_by_file, failed_files)
        if health == "green":
            green_nodes.append(node_id)
        elif health == "yellow":
            yellow_nodes.append(node_id)
        else:
            red_nodes.append(node_id)
    if green_nodes:
        lines.append(f"    class {','.join(green_nodes)} green;")
    if yellow_nodes:
        lines.append(f"    class {','.join(yellow_nodes)} yellow;")
    if red_nodes:
        lines.append(f"    class {','.join(red_nodes)} red;")

    return "\n".join(lines) + "\n"


def _render_std_graph_dot(
    dependency_graph: dict,
    trusted_by_file: dict,
    atoms_by_file: dict,
    failed_files: set,
) -> str:
    """Render the std/ dependency graph as Graphviz DOT source."""
    lines = ["digraph std_deps {", '    rankdir="TB";', '    node [shape=box, style=rounded];']
    for path in sorted(dependency_graph.keys()):
        node_id = _sanitize_node_id(path)
        atoms = atoms_by_file.get(path, 0)
        trusted = trusted_by_file.get(path, 0)
        label = f"{path}\\n{atoms} atoms, {trusted} trusted"
        health = _classify_health(path, trusted_by_file, failed_files)
        if health == "green":
            fill = "#d4edda"
            shape = "box"
            style = "rounded,filled"
        elif health == "yellow":
            fill = "#fff3cd"
            shape = "hexagon"
            style = "filled"
        else:
            fill = "#f8d7da"
            shape = "box"
            style = "rounded,filled,bold"
        lines.append(
            f'    {node_id} [label="{label}", shape={shape}, '
            f'style="{style}", fillcolor="{fill}"];',
        )
    for path in sorted(dependency_graph.keys()):
        for dep in dependency_graph[path]:
            src = _sanitize_node_id(path)
            dst = _sanitize_node_id(dep)
            lines.append(f"    {src} -> {dst};")
    lines.append("}")
    return "\n".join(lines) + "\n"


def _trusted_by_file_counts(trusted_atoms: list) -> dict:
    """Aggregate _collect_trusted_atoms() output as {rel_path: trusted_count}."""
    counts: dict = {}
    for entry in trusted_atoms:
        rel = entry.get("file", "")
        if not rel:
            continue
        counts[rel] = counts.get(rel, 0) + 1
    return counts


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


if __name__ == "__main__":
    mcp.run()
