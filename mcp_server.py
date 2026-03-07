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
            try:
                _sync_to_visualizer(report_file, root_dir)
            except Exception:
                pass

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

    Note: The verify command always writes report.json to the compiler's cwd.
    Concurrent calls may read stale data. Use forge_blade for concurrent-safe operation.
    """
    root_dir = Path(__file__).parent.absolute()

    with tempfile.TemporaryDirectory() as tmpdir:
        tmp_path = Path(tmpdir)
        source_path = tmp_path / "input.mm"
        source_path.write_text(source_code, encoding="utf-8")

        # Run mumei verify (Z3 verification only, no codegen)
        result = subprocess.run(
            ["cargo", "run", "--", "verify", str(source_path)],
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

        # TODO: The verify command writes report.json to cwd because it does not
        #   support -o. This means concurrent calls race on the same file.
        #   shutil.move (instead of copy) minimises the window, but does not
        #   eliminate it. To fully fix this, add -o / --report-dir support to
        #   `mumei verify` on the Rust side so the report can be written
        #   directly into the per-request temp directory.
        report_file = tmp_path / "report.json"
        cwd_report = root_dir / "report.json"
        if cwd_report.exists():
            try:
                shutil.move(str(cwd_report), str(report_file))
            except OSError:
                # Another request may have moved it already
                pass
        if report_file.exists():
            report_data = report_file.read_text(encoding="utf-8")
            response_parts.append(
                f"### Verification Report\n```json\n{report_data}\n```"
            )
            try:
                _sync_to_visualizer(report_file, root_dir)
            except Exception:
                pass

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

    Note: Only "build" is concurrent-safe (report isolated via -o).
    "verify" and "check" write report.json to cwd, so concurrent calls may race.
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

        result = subprocess.run(
            cmd_args,
            cwd=root_dir,
            capture_output=True,
            text=True,
            timeout=300,
        )

        response_parts = []

        # For "build", report.json is written to tmp_path (via -o).
        # TODO: For "verify"/"check", report.json is written to cwd because
        #   those commands do not support -o. shutil.move minimises the race
        #   window but does not eliminate it. To fully fix, add -o /
        #   --report-dir support to `mumei verify` and `mumei check` on the
        #   Rust side.
        report_file = tmp_path / "report.json"
        if not report_file.exists() and command != "build":
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
            try:
                _sync_to_visualizer(report_file, root_dir)
            except Exception:
                pass

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
    allowed: list[str] | None = None, denied: list[str] | None = None
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
    allowed_effects: list[str] | None = None,
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

    for attempt in range(max_attempts):
        # Run validation — this writes report.json to root_dir (compiler cwd)
        # then validate_logic moves it into a temp directory.  We run the
        # compiler directly here so we can read report.json before it is moved.
        with tempfile.TemporaryDirectory() as tmpdir:
            tmp_path = Path(tmpdir)
            source_path = tmp_path / "input.mm"
            source_path.write_text(current_code, encoding="utf-8")

            compile_result = subprocess.run(
                ["cargo", "run", "--", "verify", str(source_path)],
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
                # Reset session effects
                if allowed_effects:
                    set_allowed_effects()
                return "\n".join(results)

            results.append(f"### Attempt {attempt + 1}: FAILED")
            error_log = compile_result.stderr or compile_result.stdout or ""
            results.append(f"```\n{error_log}\n```")

            # Read report.json from compiler cwd (before it gets moved)
            report_file = root_dir / "report.json"
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
    # Reset session effects
    if allowed_effects:
        set_allowed_effects()
    return "\n".join(results)


if __name__ == "__main__":
    mcp.run()
