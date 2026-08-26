#!/usr/bin/env python3
"""Check a proof bundle against the committed ``docs/STDLIB_METRICS.md`` baseline.

``docs/STDLIB_METRICS.md`` is the committed baseline the regeneration is
compared against.  This checker is deterministic and offline: it reads JSON
and Markdown only and never invokes cargo or the mumei binary.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

MODULE_ROW_RE = re.compile(r"^\|\s*`([^`]+\.mm)`\s*\|", re.MULTILINE)
SUMMARY_RE = re.compile(
    r"Atoms total:\*\*\s*(\d+)\s*\((\d+)\s*proven\s*[·\u00b7]\s*(\d+)\s*trusted\)"
)


def _metric_modules(text: str) -> set[str]:
    return {path[:-3] for path in MODULE_ROW_RE.findall(text)}


def _metric_summary(text: str) -> tuple[int, int, int] | None:
    match = SUMMARY_RE.search(text)
    if not match:
        return None
    return tuple(int(value) for value in match.groups())


def _artifact_file(path: str, certs_dir: Path) -> Path:
    candidate = Path(path)
    if candidate.is_absolute():
        return candidate
    parts = candidate.parts
    marker = ("std", "certs")
    if len(parts) >= 3 and parts[:2] == marker:
        return certs_dir.joinpath(*parts[2:])
    return candidate


def check_drift(
    bundle_path: Path,
    metrics_path: Path,
    certs_dir: Path | None = None,
) -> dict:
    bundle = json.loads(bundle_path.read_text(encoding="utf-8"))
    metrics_text = metrics_path.read_text(encoding="utf-8")
    failures: list[str] = []
    modules = set((bundle.get("modules") or {}).keys())
    metric_modules = _metric_modules(metrics_text)

    for module in sorted(metric_modules - modules):
        failures.append(f"module_missing: metrics lists {module}.mm but bundle does not")
    for module in sorted(modules - metric_modules):
        failures.append(f"module_extra: bundle lists {module} but metrics does not")

    metric_counts = _metric_summary(metrics_text)
    summary = bundle.get("summary") or {}
    density = None
    if metric_counts is None:
        failures.append("summary_missing: metrics Summary line is absent or malformed")
    else:
        total, proven, _trusted = metric_counts
        if summary.get("total_atoms") != total:
            failures.append(
                f"atom_count_mismatch: bundle total_atoms={summary.get('total_atoms')}, "
                f"metrics total={total}"
            )
        if summary.get("proven_atoms") != proven:
            failures.append(
                f"proven_count_mismatch: bundle proven_atoms={summary.get('proven_atoms')}, "
                f"metrics proven={proven}"
            )
        density = f"{proven}/{total}"

    for index, entry in enumerate(bundle.get("lean_provenance") or []):
        if not entry.get("translator_version") or not entry.get("bridge_lemma_hash"):
            failures.append(
                f"stale_translator: lean_provenance[{index}] has empty "
                "translator_version or bridge_lemma_hash"
            )

    artifact_paths = list(bundle.get("artifact_paths") or [])
    if certs_dir is not None:
        for path in artifact_paths:
            artifact = _artifact_file(path, certs_dir)
            if not artifact.is_file():
                failures.append(f"artifact_missing: {path}")

    return {
        "ok": not failures,
        "failures": failures,
        "artifact_paths": artifact_paths,
        "proof_density": density,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--metrics", type=Path, required=True)
    parser.add_argument("--certs-dir", type=Path)
    parser.add_argument("--json", action="store_true", dest="as_json")
    args = parser.parse_args(argv)
    try:
        result = check_drift(args.bundle, args.metrics, args.certs_dir)
    except (OSError, json.JSONDecodeError, TypeError, ValueError) as exc:
        result = {
            "ok": False,
            "failures": [f"input_error: {exc}"],
            "artifact_paths": [],
            "proof_density": None,
        }
    if args.as_json:
        print(json.dumps(result, indent=2, ensure_ascii=False))
    elif result["ok"]:
        print("proof bundle drift check passed")
        print("artifact_paths:")
        for path in result["artifact_paths"]:
            print(f"- {path}")
        if result["proof_density"] is not None:
            print(f"proof_density: {result['proof_density']}")
    else:
        print("proof bundle drift check failed:", file=sys.stderr)
        for failure in result["failures"]:
            print(f"- {failure}", file=sys.stderr)
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
