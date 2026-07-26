"""Benchmark evaluation suite for mumei verification pipeline.

Collects:
- Expected-outcome match rate per benchmark category (success examples and
  counterexample cases marked ``expected: FAIL``)
- Counterexample catch rate: how many bug-carrying cases the verifier rejects
- Z3 solver time per atom
- Lean escalation solver time for Z3 ``unknown`` obligations (P16-B); degrades to
  ``SKIP`` at zero cost when the mumei-lean bridge is unavailable
- Trusted atom ratio across the stdlib
- Time-series append to ``docs/BENCHMARK_RESULTS.md``

Usage::

    python benchmarks/run_benchmarks.py [--output docs/BENCHMARK_RESULTS.md]
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
BENCHMARKS_DIR = REPO_ROOT / "benchmarks"
STD_DIR = REPO_ROOT / "std"
METRICS_OUTPUT = REPO_ROOT / "docs" / "BENCHMARK_RESULTS.md"

CATEGORIES = {
    "arithmetic": BENCHMARKS_DIR / "arithmetic",
    "concurrency": BENCHMARKS_DIR / "concurrency",
    "dafny_puzzles": BENCHMARKS_DIR / "dafny_puzzles",
    "domain_compliance": BENCHMARKS_DIR / "domain_compliance",
    "state_machine": BENCHMARKS_DIR / "state_machine",
    "svcomp_style": BENCHMARKS_DIR / "svcomp_style",
}

# Benchmark category -> stdlib domains whose forge/proliferate targets the
# category exercises. Used to turn category-level weakness into a priority bias
# for the vStd forge pipeline (see docs/BENCHMARK_RESULTS.md).
CATEGORY_STD_DOMAINS = {
    "arithmetic": ["std/math", "std/algebra"],
    "concurrency": ["std/concurrency"],
    "dafny_puzzles": ["std/list", "std/container", "std/iter"],
    "domain_compliance": ["std/compliance", "std/settlement"],
    "state_machine": ["std/contracts", "std/settlement"],
    "svcomp_style": ["std/core", "std/bitwise"],
}

FORGE_FEEDBACK_SCHEMA = "mumei.benchmark_forge_feedback/v1"
# Weakness weights: outcome mismatches dominate, missed counterexamples next,
# residual trusted atoms last.
WEAKNESS_WEIGHTS = {"success": 0.5, "counterexample": 0.3, "trusted": 0.2}
# Priority is "lower runs first" in the forge task queue, so the bias is negative.
MAX_PRIORITY_BOOST = 50
SLOW_SOLVER_TIME_S = 5.0
SLOW_LEAN_SOLVER_TIME_S = 60.0

# A benchmark file is a counterexample case when its name ends with ``_fail`` or
# its leading comment block declares ``expected: FAIL``.
EXPECTED_FAIL_RE = re.compile(r"expected:\s*FAIL", re.IGNORECASE)
ESCALATION_CANDIDATE_RE = re.compile(r"(\d+)\s+Lean escalation candidate")
LEAN_VERIFY_TIMEOUT_S = 300


def _find_mumei_binary() -> str | None:
    for candidate in [
        str(REPO_ROOT / "target" / "release" / "mumei"),
        str(REPO_ROOT / "target" / "debug" / "mumei"),
    ]:
        if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return candidate
    return None


def _resolve_lean_bridge() -> Path | None:
    """Locate ``scripts/bridge.py`` in the mumei-lean repository.

    Mirrors the resolution order of ``resolve_mumei_lean_bridge`` in
    ``src/commands/verify.rs`` so that the benchmark suite and the CLI agree on
    whether Lean escalation is available.
    """
    configured = os.environ.get("MUMEI_LEAN_PATH")
    candidates: list[Path] = []
    if configured:
        path = Path(configured)
        candidates.append(path if path.is_file() else path / "scripts" / "bridge.py")
    else:
        candidates.extend([
            REPO_ROOT.parent / "mumei-lean" / "scripts" / "bridge.py",
            REPO_ROOT / "mumei-lean" / "scripts" / "bridge.py",
        ])
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    return None


def _expected_outcome(path: Path) -> str:
    """Return ``"FAIL"`` for counterexample cases, ``"PASS"`` otherwise."""
    if path.stem.endswith("_fail"):
        return "FAIL"
    header: list[str] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped and not stripped.startswith("//"):
            break
        header.append(stripped)
    return "FAIL" if EXPECTED_FAIL_RE.search("\n".join(header)) else "PASS"


def _count_atoms(path: Path) -> dict[str, int]:
    text = path.read_text(encoding="utf-8")
    total = len(re.findall(r"(?m)^\s*(?:trusted\s+|async\s+)?atom\s+", text))
    trusted = len(re.findall(r"(?m)^\s*trusted\s+atom\s+", text))
    return {"total": total, "trusted": trusted, "proven": total - trusted}


def _verify_file(
    binary: str,
    path: Path,
    expected: str,
    timeout: int = 120,
    lean_bridge: Path | None = None,
) -> dict:
    """Verify one benchmark file and compare the outcome against ``expected``.

    ``lean_solver_time_s`` is populated only when the file leaves Z3 ``unknown``
    obligations *and* the mumei-lean bridge is available; otherwise the Lean
    escalation measurement is skipped without spawning any extra process.
    """
    start = time.monotonic()
    stdout = ""
    try:
        proc = subprocess.run(
            [binary, "verify", str(path)],
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=str(REPO_ROOT),
        )
        elapsed = time.monotonic() - start
        ok = proc.returncode == 0
        stdout = proc.stdout
    except subprocess.TimeoutExpired:
        elapsed = float(timeout)
        ok = False
    except FileNotFoundError:
        elapsed = 0.0
        ok = False

    actual = "PASS" if ok else "FAIL"
    result = {
        "ok": ok,
        "elapsed_s": round(elapsed, 3),
        "actual": actual,
        "expected": expected,
        "matched": actual == expected,
        "escalation_candidates": _escalation_candidate_count(stdout),
        "lean_solver_time_s": None,
        "lean_status": "SKIP",
    }
    if result["escalation_candidates"] and lean_bridge is not None:
        lean = _measure_lean_escalation(binary, path)
        result["lean_solver_time_s"] = lean["lean_solver_time_s"]
        result["lean_status"] = lean["lean_status"]
    return result


def _escalation_candidate_count(stdout: str) -> int:
    match = ESCALATION_CANDIDATE_RE.search(stdout)
    return int(match.group(1)) if match else 0


def _measure_lean_escalation(binary: str, path: Path) -> dict:
    """Time the mumei-lean escalation of a file's Z3 ``unknown`` obligations."""
    start = time.monotonic()
    try:
        proc = subprocess.run(
            [binary, "verify", "--proof-cert", "--escalate-lean", str(path)],
            capture_output=True,
            text=True,
            timeout=LEAN_VERIFY_TIMEOUT_S,
            cwd=str(REPO_ROOT),
        )
        elapsed = time.monotonic() - start
        status = "MEASURED" if proc.returncode == 0 else "FAIL"
    except subprocess.TimeoutExpired:
        elapsed = float(LEAN_VERIFY_TIMEOUT_S)
        status = "TIMEOUT"
    except FileNotFoundError:
        return {"lean_solver_time_s": None, "lean_status": "SKIP"}
    return {"lean_solver_time_s": round(elapsed, 3), "lean_status": status}


def run_category_benchmarks(
    binary: str | None,
    category: str,
    dir_path: Path,
    lean_bridge: Path | None = None,
) -> dict:
    results: list[dict] = []
    for mm_file in sorted(dir_path.glob("*.mm")):
        counts = _count_atoms(mm_file)
        expected = _expected_outcome(mm_file)
        if binary:
            verify = _verify_file(binary, mm_file, expected, lean_bridge=lean_bridge)
        else:
            verify = {
                "ok": None,
                "elapsed_s": 0.0,
                "actual": "SKIP",
                "expected": expected,
                "matched": False,
                "escalation_candidates": 0,
                "lean_solver_time_s": None,
                "lean_status": "SKIP",
            }
        results.append({
            "file": mm_file.name,
            "atoms": counts["total"],
            "trusted": counts["trusted"],
            "proven": counts["proven"],
            "verified": verify["ok"],
            "expected": verify["expected"],
            "actual": verify["actual"],
            "matched": verify["matched"],
            "solver_time_s": verify["elapsed_s"],
            "escalation_candidates": verify["escalation_candidates"],
            "lean_solver_time_s": verify["lean_solver_time_s"],
            "lean_status": verify["lean_status"],
        })
    total_atoms = sum(r["atoms"] for r in results)
    total_trusted = sum(r["trusted"] for r in results)
    verified_count = sum(1 for r in results if r["verified"] is True)
    matched_count = sum(1 for r in results if r["matched"])
    counterexamples = [r for r in results if r["expected"] == "FAIL"]
    counterexamples_caught = sum(1 for r in counterexamples if r["matched"])
    lean_times = [
        r["lean_solver_time_s"] for r in results if r["lean_solver_time_s"] is not None
    ]
    return {
        "category": category,
        "files": len(results),
        "total_atoms": total_atoms,
        "total_trusted": total_trusted,
        "trusted_ratio": round(total_trusted / total_atoms, 4) if total_atoms else 0.0,
        "verified_count": verified_count,
        "matched_count": matched_count,
        # Generalized: rate of files whose outcome matched the expected outcome.
        "success_rate": round(matched_count / len(results), 4) if results else 0.0,
        "counterexample_files": len(counterexamples),
        "counterexamples_caught": counterexamples_caught,
        "counterexample_catch_rate": round(
            counterexamples_caught / len(counterexamples), 4
        ) if counterexamples else None,
        "lean_measured_files": len(lean_times),
        "avg_lean_solver_time_s": round(sum(lean_times) / len(lean_times), 3)
        if lean_times else None,
        "avg_solver_time_s": round(
            sum(r["solver_time_s"] for r in results) / len(results), 3
        ) if results else 0.0,
        "details": results,
    }


def collect_stdlib_metrics() -> dict:
    total_atoms = 0
    total_trusted = 0
    module_count = 0
    for mm_file in sorted(STD_DIR.rglob("*.mm")):
        counts = _count_atoms(mm_file)
        total_atoms += counts["total"]
        total_trusted += counts["trusted"]
        module_count += 1
    return {
        "modules": module_count,
        "total_atoms": total_atoms,
        "total_trusted": total_trusted,
        "trusted_ratio": round(total_trusted / total_atoms, 4) if total_atoms else 0.0,
        "proven": total_atoms - total_trusted,
    }


def _category_signals(cat: dict) -> list[str]:
    signals = []
    if cat["success_rate"] < 1.0:
        signals.append("expected_outcome_mismatch")
    catch_rate = cat["counterexample_catch_rate"]
    if catch_rate is not None and catch_rate < 1.0:
        signals.append("counterexample_missed")
    if cat["trusted_ratio"] > 0.0:
        signals.append("trusted_atoms_present")
    if cat["avg_solver_time_s"] > SLOW_SOLVER_TIME_S:
        signals.append("z3_solver_time_pressure")
    lean_time = cat["avg_lean_solver_time_s"]
    if lean_time is not None and lean_time > SLOW_LEAN_SOLVER_TIME_S:
        signals.append("lean_escalation_cost")
    return signals


def _weakness_score(cat: dict) -> float:
    catch_rate = cat["counterexample_catch_rate"]
    catch_gap = 0.0 if catch_rate is None else 1.0 - catch_rate
    score = (
        WEAKNESS_WEIGHTS["success"] * (1.0 - cat["success_rate"])
        + WEAKNESS_WEIGHTS["counterexample"] * catch_gap
        + WEAKNESS_WEIGHTS["trusted"] * cat["trusted_ratio"]
    )
    return round(score, 4)


def build_forge_feedback(
    timestamp: str,
    category_results: list[dict],
    stdlib_metrics: dict,
) -> dict:
    """Project benchmark results into the vStd forge / proliferate input contract.

    Each category contributes a ``weakness_score`` in ``[0, 1]`` derived from its
    expected-outcome success rate, counterexample catch rate, and trusted ratio,
    plus solver-time signals. The score is mapped to a negative
    ``priority_delta`` (the forge queue runs lower priorities first) for the
    stdlib domains the category exercises, so weak categories pull their
    stdlib modules forward in the proliferation queue.
    """
    categories = []
    domain_bias: dict[str, dict] = {}
    for cat in category_results:
        score = _weakness_score(cat)
        priority_delta = -int(round(score * MAX_PRIORITY_BOOST))
        domains = CATEGORY_STD_DOMAINS.get(cat["category"], [])
        categories.append({
            "category": cat["category"],
            "files": cat["files"],
            "success_rate": cat["success_rate"],
            "counterexample_catch_rate": cat["counterexample_catch_rate"],
            "trusted_ratio": cat["trusted_ratio"],
            "avg_solver_time_s": cat["avg_solver_time_s"],
            "avg_lean_solver_time_s": cat["avg_lean_solver_time_s"],
            "lean_measured_files": cat["lean_measured_files"],
            "weakness_score": score,
            "signals": _category_signals(cat),
            "std_domains": domains,
            "priority_delta": priority_delta,
        })
        for domain in domains:
            current = domain_bias.get(domain)
            if current is None or priority_delta < current["priority_delta"]:
                domain_bias[domain] = {
                    "domain": domain,
                    "priority_delta": priority_delta,
                    "weakness_score": score,
                    "driving_category": cat["category"],
                }
    weak = sorted(
        (c for c in categories if c["weakness_score"] > 0.0),
        key=lambda c: (-c["weakness_score"], c["category"]),
    )
    return {
        "schema": FORGE_FEEDBACK_SCHEMA,
        "timestamp": timestamp,
        "stdlib_trusted_ratio": stdlib_metrics["trusted_ratio"],
        "categories": categories,
        "weak_categories": [c["category"] for c in weak],
        "domain_bias": [domain_bias[k] for k in sorted(domain_bias)],
    }


def _fmt_rate(rate: float | None) -> str:
    return "n/a" if rate is None else f"{rate:.2%}"


def _fmt_lean_time(seconds: float | None) -> str:
    return "SKIP" if seconds is None else f"{seconds:.3f}s"


def format_report(
    timestamp: str,
    category_results: list[dict],
    stdlib_metrics: dict,
) -> str:
    lines = [
        f"## Benchmark Run — {timestamp}",
        "",
        "### Stdlib Health Summary",
        "",
        f"| Modules | Atoms | Proven | Trusted | Trusted Ratio |",
        f"|---------|-------|--------|---------|---------------|",
        f"| {stdlib_metrics['modules']} | {stdlib_metrics['total_atoms']} "
        f"| {stdlib_metrics['proven']} | {stdlib_metrics['total_trusted']} "
        f"| {stdlib_metrics['trusted_ratio']:.4f} |",
        "",
        "### Category Results",
        "",
        "Success Rate is the share of files whose verification outcome matched the",
        "expected outcome (`expected: PASS` or `expected: FAIL`). Counterexample Catch",
        "is the share of `expected: FAIL` files the verifier correctly rejected.",
        "",
        "| Category | Files | Atoms | Trusted | Success Rate | Counterexample Catch | Avg Solver Time | Avg Lean Solver Time |",
        "|----------|-------|-------|---------|--------------|----------------------|-----------------|----------------------|",
    ]
    for cat in category_results:
        lines.append(
            f"| {cat['category']} | {cat['files']} | {cat['total_atoms']} "
            f"| {cat['total_trusted']} | {cat['success_rate']:.2%} "
            f"| {_fmt_rate(cat['counterexample_catch_rate'])} "
            f"({cat['counterexamples_caught']}/{cat['counterexample_files']}) "
            f"| {cat['avg_solver_time_s']:.3f}s "
            f"| {_fmt_lean_time(cat['avg_lean_solver_time_s'])} |"
        )
    lines.append("")
    lines.append("<details><summary>Per-file details</summary>")
    lines.append("")
    for cat in category_results:
        lines.append(f"#### {cat['category']}")
        lines.append("")
        lines.append(
            "| File | Atoms | Trusted | Expected | Actual | Match | Solver Time | Lean Solver Time |"
        )
        lines.append(
            "|------|-------|---------|----------|--------|-------|-------------|------------------|"
        )
        for d in cat["details"]:
            lines.append(
                f"| {d['file']} | {d['atoms']} | {d['trusted']} "
                f"| {d['expected']} | {d['actual']} | {'yes' if d['matched'] else 'no'} "
                f"| {d['solver_time_s']:.3f}s | {_fmt_lean_time(d['lean_solver_time_s'])} |"
            )
        lines.append("")
    lines.append("</details>")
    lines.append("")
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(description="Run mumei benchmark suite")
    parser.add_argument(
        "--output",
        type=str,
        default=str(METRICS_OUTPUT),
        help="Output path for benchmark results markdown",
    )
    parser.add_argument(
        "--json",
        type=str,
        default=None,
        help="Also write results as JSON to the given path",
    )
    parser.add_argument(
        "--forge-feedback",
        type=str,
        default=None,
        help=(
            "Write a " + FORGE_FEEDBACK_SCHEMA + " document to the given path for "
            "the mumei-agent vStd forge / proliferate pipeline"
        ),
    )
    parser.add_argument(
        "--no-lean",
        action="store_true",
        help="Never invoke the mumei-lean bridge; report Lean times as SKIP",
    )
    args = parser.parse_args()

    binary = _find_mumei_binary()
    if not binary:
        print("mumei binary not found; solver times will be skipped", file=sys.stderr)

    lean_bridge = None if args.no_lean else _resolve_lean_bridge()
    if lean_bridge is None:
        print(
            "mumei-lean bridge not available; Lean escalation times will be SKIP",
            file=sys.stderr,
        )

    timestamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M UTC")

    category_results = []
    for name, dir_path in sorted(CATEGORIES.items()):
        if dir_path.is_dir():
            category_results.append(
                run_category_benchmarks(binary, name, dir_path, lean_bridge=lean_bridge)
            )

    stdlib_metrics = collect_stdlib_metrics()

    report = format_report(timestamp, category_results, stdlib_metrics)

    output_path = Path(args.output)
    header = "# Benchmark Results\n\nTime-series benchmark results for the mumei verification pipeline.\n\n"
    if output_path.exists():
        existing = output_path.read_text(encoding="utf-8")
        if existing.startswith("# Benchmark Results"):
            content = existing + "\n---\n\n" + report
        else:
            content = header + report
    else:
        content = header + report
    output_path.write_text(content, encoding="utf-8")
    print(f"wrote {output_path}")

    if args.json:
        json_data = {
            "timestamp": timestamp,
            "stdlib": stdlib_metrics,
            "categories": category_results,
        }
        json_path = Path(args.json)
        json_path.write_text(json.dumps(json_data, indent=2), encoding="utf-8")
        print(f"wrote {json_path}")

    if args.forge_feedback:
        feedback = build_forge_feedback(timestamp, category_results, stdlib_metrics)
        feedback_path = Path(args.forge_feedback)
        feedback_path.write_text(json.dumps(feedback, indent=2), encoding="utf-8")
        print(f"wrote {feedback_path}")


if __name__ == "__main__":
    main()
