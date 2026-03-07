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

# Visualizer sync config
# true: also copy report.json to visualizer/ (for Streamlit dashboard)
# false: MCP response only (default)
VISUALIZER_SYNC = os.getenv("ENABLE_VISUALIZER_SYNC", "false").lower() == "true"
HISTORY_FILE = Path(__file__).parent.absolute() / "visualizer" / "report_history.json"


def _sync_to_visualizer(report_file: Path, root_dir: Path) -> None:
    """Copy report.json to visualizer/ and append to history."""
    if not VISUALIZER_SYNC:
        return
    if not report_file.exists():
        return

    vis_dir = root_dir / "visualizer"
    vis_dir.mkdir(exist_ok=True)
    shutil.copy(report_file, vis_dir / "report.json")

    # Append to history
    import datetime
    entry = json.loads(report_file.read_text(encoding="utf-8"))
    entry["timestamp"] = datetime.datetime.now(datetime.timezone.utc).isoformat()

    history = []
    if HISTORY_FILE.exists():
        try:
            history = json.loads(HISTORY_FILE.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            history = []
    history.append(entry)
    HISTORY_FILE.write_text(json.dumps(history, indent=2, ensure_ascii=False), encoding="utf-8")

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

        # report.json is written to output_dir (parent of -o path) = tmp_path
        report_file = tmp_path / "report.json"
        if report_file.exists():
            report_data = report_file.read_text(encoding="utf-8")
            response_parts.append(f"### Verification Report\n```json\n{report_data}\n```")
            _sync_to_visualizer(report_file, root_dir)

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
            _sync_to_visualizer(report_file, root_dir)

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
            text=True
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
            _sync_to_visualizer(report_file, root_dir)

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


if __name__ == "__main__":
    mcp.run()
