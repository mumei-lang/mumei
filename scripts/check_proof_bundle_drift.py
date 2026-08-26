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


def _density(proven: int, total: int) -> tuple[str, float]:
    if total == 0:
        return "0/0", 0.0
    return f"{proven}/{total}", proven / total


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
    bundle_density = None
    bundle_density_value = None
    metrics_density = None
    metrics_density_value = None
    if metric_counts is None:
        failures.append("summary_missing: metrics Summary line is absent or malformed")
    else:
        metric_total, metric_proven, metric_trusted = metric_counts
        metrics_density, metrics_density_value = _density(
            metric_proven, metric_total
        )
        bundle_total = summary.get("total_atoms")
        bundle_proven = summary.get("proven_atoms")
        bundle_trusted = summary.get("trusted_atoms")
        if not isinstance(bundle_total, int) or not isinstance(bundle_proven, int):
            failures.append(
                "summary_missing: bundle summary must contain integer "
                "total_atoms and proven_atoms"
            )
        else:
            bundle_density, bundle_density_value = _density(
                bundle_proven, bundle_total
            )
        if bundle_trusted != metric_trusted:
            failures.append(
                f"trusted_count_mismatch: bundle trusted_atoms={bundle_trusted}, "
                f"metrics trusted={metric_trusted}"
            )
        if (
            bundle_density_value is not None
            and metrics_density_value is not None
            and bundle_density_value < metrics_density_value
        ):
            failures.append(
                f"proof_density_regression: bundle {bundle_density} is below "
                f"metrics {metrics_density}"
            )

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
        "proof_density": bundle_density,
        "bundle_proof_density": bundle_density,
        "metrics_proof_density": metrics_density,
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
    else:
        print("proof bundle drift check failed:", file=sys.stderr)
        for failure in result["failures"]:
            print(f"- {failure}", file=sys.stderr)
    if not args.as_json:
        if result["bundle_proof_density"] is not None:
            print(f"bundle_proof_density: {result['bundle_proof_density']}")
        if result["metrics_proof_density"] is not None:
            print(f"metrics_proof_density: {result['metrics_proof_density']}")
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
