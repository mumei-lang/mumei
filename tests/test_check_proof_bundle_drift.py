from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CHECKER = ROOT / "scripts" / "check_proof_bundle_drift.py"
METRICS = """\
# Metrics

## Summary
- **Atoms total:** 3 (2 proven · 1 trusted)

## Per-module breakdown
| Module | Atoms | Trusted | TODOs | Verification | Health |
| --- | ---: | ---: | ---: | --- | ---: |
| `std/core.mm` | 2 | 1 | 0 | OK | 1.000 |
| `std/io.mm` | 1 | 0 | 0 | OK | 1.000 |
"""


def _bundle(tmp_path: Path) -> dict:
    return {
        "bundle_version": "1.1",
        "modules": {"std/core": {}, "std/io": {}},
        "artifact_paths": ["std/certs/core.proof.json"],
        "lean_provenance": [
            {
                "module": "std/core",
                "atom": "a",
                "z3_check_result": "lean_verified",
                "translator_version": "v1",
                "bridge_lemma_hash": "hash",
                "manual_lemma_reason": None,
            }
        ],
        "summary": {"total_atoms": 3, "proven_atoms": 2},
    }


def _run(tmp_path: Path, bundle: dict, metrics: str = METRICS) -> subprocess.CompletedProcess[str]:
    bundle_path = tmp_path / "bundle.json"
    metrics_path = tmp_path / "metrics.md"
    bundle_path.write_text(json.dumps(bundle), encoding="utf-8")
    metrics_path.write_text(metrics, encoding="utf-8")
    return subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            "--bundle",
            str(bundle_path),
            "--metrics",
            str(metrics_path),
        ],
        capture_output=True,
        text=True,
        check=False,
    )


def test_passing_fixture_prints_artifact_paths(tmp_path: Path) -> None:
    result = _run(tmp_path, _bundle(tmp_path))
    assert result.returncode == 0
    assert "std/certs/core.proof.json" in result.stdout


def test_module_missing_from_bundle(tmp_path: Path) -> None:
    bundle = _bundle(tmp_path)
    del bundle["modules"]["std/io"]
    result = _run(tmp_path, bundle)
    assert result.returncode == 1
    assert "module_missing" in result.stderr


def test_atom_count_mismatch(tmp_path: Path) -> None:
    bundle = _bundle(tmp_path)
    bundle["summary"]["total_atoms"] = 4
    result = _run(tmp_path, bundle)
    assert result.returncode == 1
    assert "atom_count_mismatch" in result.stderr


def test_empty_bridge_hash_is_stale_translator(tmp_path: Path) -> None:
    bundle = _bundle(tmp_path)
    bundle["lean_provenance"][0]["bridge_lemma_hash"] = ""
    result = _run(tmp_path, bundle)
    assert result.returncode == 1
    assert "stale_translator" in result.stderr


def test_missing_artifact_path(tmp_path: Path) -> None:
    bundle = _bundle(tmp_path)
    result = _run(tmp_path, bundle)
    # Supplying --certs-dir makes artifact_paths part of the check.
    bundle_path = tmp_path / "bundle.json"
    metrics_path = tmp_path / "metrics.md"
    bundle_path.write_text(json.dumps(bundle), encoding="utf-8")
    metrics_path.write_text(METRICS, encoding="utf-8")
    result = subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            "--bundle",
            str(bundle_path),
            "--metrics",
            str(metrics_path),
            "--certs-dir",
            str(tmp_path / "certs"),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 1
    assert "artifact_missing" in result.stderr


def test_metrics_check_without_mumei_binary(tmp_path: Path) -> None:
    std_dir = tmp_path / "std"
    std_dir.mkdir()
    (std_dir / "sample.mm").write_text("atom sample() ensures true { true }\n", encoding="utf-8")
    report = tmp_path / "metrics.md"
    generate = subprocess.run(
        [
            sys.executable,
            str(ROOT / "scripts" / "generate_stdlib_metrics.py"),
            "--std-dir",
            str(std_dir),
            "--output",
            str(report),
            "--no-history",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert generate.returncode == 0, generate.stderr
    check = subprocess.run(
        [
            sys.executable,
            str(ROOT / "scripts" / "generate_stdlib_metrics.py"),
            "--std-dir",
            str(std_dir),
            "--output",
            str(report),
            "--check",
            "--no-history",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert check.returncode == 0, check.stderr
