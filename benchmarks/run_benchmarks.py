"""Benchmark evaluation suite for mumei verification pipeline.

Collects:
- Verification success rate per benchmark category
- Z3 solver time per atom
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
    "dafny_puzzles": BENCHMARKS_DIR / "dafny_puzzles",
    "svcomp_style": BENCHMARKS_DIR / "svcomp_style",
}


def _find_mumei_binary() -> str | None:
    for candidate in [
        str(REPO_ROOT / "target" / "release" / "mumei"),
        str(REPO_ROOT / "target" / "debug" / "mumei"),
    ]:
        if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return candidate
    return None


def _count_atoms(path: Path) -> dict[str, int]:
    text = path.read_text(encoding="utf-8")
    total = len(re.findall(r"(?m)^\s*(?:trusted\s+)?atom\s+", text))
    trusted = len(re.findall(r"(?m)^\s*trusted\s+atom\s+", text))
    return {"total": total, "trusted": trusted, "proven": total - trusted}


def _verify_file(binary: str, path: Path, timeout: int = 30) -> dict:
    start = time.monotonic()
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
    except subprocess.TimeoutExpired:
        elapsed = float(timeout)
        ok = False
    except FileNotFoundError:
        elapsed = 0.0
        ok = False
    return {"ok": ok, "elapsed_s": round(elapsed, 3)}


def run_category_benchmarks(binary: str | None, category: str, dir_path: Path) -> dict:
    results: list[dict] = []
    for mm_file in sorted(dir_path.glob("*.mm")):
        counts = _count_atoms(mm_file)
        if binary:
            verify = _verify_file(binary, mm_file)
        else:
            verify = {"ok": None, "elapsed_s": 0.0}
        results.append({
            "file": mm_file.name,
            "atoms": counts["total"],
            "trusted": counts["trusted"],
            "proven": counts["proven"],
            "verified": verify["ok"],
            "solver_time_s": verify["elapsed_s"],
        })
    total_atoms = sum(r["atoms"] for r in results)
    total_trusted = sum(r["trusted"] for r in results)
    verified_count = sum(1 for r in results if r["verified"] is True)
    return {
        "category": category,
        "files": len(results),
        "total_atoms": total_atoms,
        "total_trusted": total_trusted,
        "trusted_ratio": round(total_trusted / total_atoms, 4) if total_atoms else 0.0,
        "verified_count": verified_count,
        "success_rate": round(verified_count / len(results), 4) if results else 0.0,
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
        "| Category | Files | Atoms | Trusted | Success Rate | Avg Solver Time |",
        "|----------|-------|-------|---------|--------------|-----------------|",
    ]
    for cat in category_results:
        lines.append(
            f"| {cat['category']} | {cat['files']} | {cat['total_atoms']} "
            f"| {cat['total_trusted']} | {cat['success_rate']:.2%} "
            f"| {cat['avg_solver_time_s']:.3f}s |"
        )
    lines.append("")
    lines.append("<details><summary>Per-file details</summary>")
    lines.append("")
    for cat in category_results:
        lines.append(f"#### {cat['category']}")
        lines.append("")
        lines.append("| File | Atoms | Trusted | Verified | Solver Time |")
        lines.append("|------|-------|---------|----------|-------------|")
        for d in cat["details"]:
            status = "PASS" if d["verified"] is True else ("FAIL" if d["verified"] is False else "SKIP")
            lines.append(
                f"| {d['file']} | {d['atoms']} | {d['trusted']} "
                f"| {status} | {d['solver_time_s']:.3f}s |"
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
    args = parser.parse_args()

    binary = _find_mumei_binary()
    if not binary:
        print("mumei binary not found; solver times will be skipped", file=sys.stderr)

    timestamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M UTC")

    category_results = []
    for name, dir_path in sorted(CATEGORIES.items()):
        if dir_path.is_dir():
            category_results.append(run_category_benchmarks(binary, name, dir_path))

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


if __name__ == "__main__":
    main()
