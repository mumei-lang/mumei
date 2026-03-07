import subprocess
import json
import os
import re
import shutil
import time
import datetime
from pathlib import Path
from openai import OpenAI
from dotenv import load_dotenv

# Load environment variables from .env file
load_dotenv()

# LLM provider config (supports Qwen3.5 / Ollama / vLLM / OpenAI)
api_key = os.getenv("LLM_API_KEY", os.getenv("OPENAI_API_KEY", ""))
base_url = os.getenv("LLM_BASE_URL", None)  # None defaults to OpenAI
model = os.getenv("LLM_MODEL", "gpt-4o")

if not api_key:
    raise ValueError(
        "LLM_API_KEY (or OPENAI_API_KEY) is not set. "
        "Please check your .env file."
    )

# Initialize OpenAI-compatible client (supports Ollama / vLLM / external APIs)
client_kwargs = {"api_key": api_key}
if base_url:
    client_kwargs["base_url"] = base_url

client = OpenAI(**client_kwargs)

SOURCE_FILE = "sword_test.mm"
OUTPUT_BASE = "katana"
REPORT_FILE = "report.json"  # matches output_dir (current directory)
MAX_RETRIES = 5  # max fix attempts

# Visualizer sync config
VISUALIZER_SYNC = os.getenv("ENABLE_VISUALIZER_SYNC", "false").lower() == "true"
ROOT_DIR = Path(__file__).parent.absolute()
HISTORY_FILE = ROOT_DIR / "visualizer" / "report_history.json"


def sync_to_visualizer(report_path: str) -> None:
    """Copy report.json to visualizer/ and append to history.

    NOTE: Nearly identical logic exists in mcp_server.py (_sync_to_visualizer).
    If you change this, update mcp_server.py as well (or extract into a shared
    module in the future).
    """
    if not VISUALIZER_SYNC:
        return
    report_file = Path(report_path)
    if not report_file.exists():
        return

    vis_dir = ROOT_DIR / "visualizer"
    vis_dir.mkdir(exist_ok=True)
    shutil.copy(report_file, vis_dir / "report.json")

    # Append to history (with file lock to prevent corruption when
    # mcp_server.py and self_healing.py run concurrently)
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

def run_mumei():
    """Run compiler. Detects failure via non-zero exit code."""
    result = subprocess.run(
        ["cargo", "run", "--", "build", SOURCE_FILE, "-o", OUTPUT_BASE],
        capture_output=True, text=True
    )
    # Non-zero returncode means failure
    return result.returncode == 0, result.stdout + result.stderr

def get_fix_from_ai(source_code, error_log, report_data):
    """Send error details and verification report (with counter-examples) to AI and get a fix."""
    prompt = f"""
You are an expert in the Mumei language. The following code failed formal verification.
Please fix the 'requires' (precondition) to resolve the mathematical contradiction.

# Source code:
{source_code}

# Error log:
{error_log}

# Verification report (counter-example data):
{json.dumps(report_data, indent=2)}

Output only the fixed code in ```rust ... ``` format.
"""
    response = client.chat.completions.create(
        model=model,
        messages=[{"role": "system", "content": "You are a helpful programming assistant."},
                  {"role": "user", "content": prompt}]
    )

    content = response.choices[0].message.content or ""
    # Extract code block (handles various LLM fence labels:
    #   ```rust, ```rs, ```Rust, ```mumei, ```mm, ``` (no tag), etc.)
    code_match = re.search(
        r'```(?:rust|rs|Rust|RS|mumei|mm|Mumei)?\s*\n(.*?)```',
        content,
        re.DOTALL,
    )
    if code_match:
        return code_match.group(1).strip()
    # Fallback: return raw content if no code block found
    return content.strip()

def main():
    print("Mumei Self-Healing Loop Start...")

    for attempt in range(MAX_RETRIES):
        success, logs = run_mumei()

        if success:
            print(f"Success! Blade is flawless (Attempt {attempt + 1}).")

            return

        print(f"Attempt {attempt + 1}: Flaw detected. Consulting AI...")

        # Read the latest verification report
        try:
            with open(REPORT_FILE, "r") as f:
                report = json.load(f)
        except Exception:
            report = {"status": "error", "reason": "Report not found"}

        # Visualizer sync (separate try so a sync failure doesn't mask report data)
        try:
            sync_to_visualizer(REPORT_FILE)
        except Exception:
            pass

        with open(SOURCE_FILE, "r") as f:
            source = f.read()

        # Get fix from AI
        fixed_code = get_fix_from_ai(source, logs, report)

        # Overwrite source file
        with open(SOURCE_FILE, "w") as f:
            f.write(fixed_code)

        print("Code updated. Retrying...")
        time.sleep(2)

    print("Healing failed. The blade remains broken.")

if __name__ == "__main__":
    main()
