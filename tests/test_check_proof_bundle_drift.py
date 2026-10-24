from __future__ import annotations

import json
import stat
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CHECKER = ROOT / "scripts" / "check_proof_bundle_drift.py"
PACKAGED_CHECKER = ROOT / "scripts" / "verify_packaged_certs.py"
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
        "modules": {
            "std/core": {
                "atoms": [
                    {"name": "proven", "status": "verified", "z3_check_result": "unsat"},
                    {"name": "trusted", "status": "trusted", "z3_check_result": "skipped"},
                ]
            },
            "std/io": {
                "atoms": [
                    {"name": "io", "status": "verified", "z3_check_result": "unsat"},
                ]
            },
        },
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
        "summary": {
            "total_modules": 2,
            "total_atoms": 3,
            "proven_atoms": 2,
            "lean_verified_atoms": 0,
            "trusted_atoms": 1,
        },
    }


def _baseline(
    *,
    trusted: dict[str, list[str]] | None = None,
    include_trusted: bool = True,
) -> dict:
    baseline = {
        "total_modules": 2,
        "total_atoms": 3,
        "proven_atoms": 2,
        "lean_verified_atoms": 0,
        "trusted_atoms": 1,
        "modules": {},
    }
    if include_trusted:
        baseline["trusted"] = (
            trusted if trusted is not None else {"std/core": ["trusted"]}
        )
    return baseline


def _run(
    tmp_path: Path,
    bundle: dict,
    metrics: str = METRICS,
    *,
    baseline: dict | None = None,
    as_json: bool = False,
) -> subprocess.CompletedProcess[str]:
    tmp_path.mkdir(parents=True, exist_ok=True)
    bundle_path = tmp_path / "bundle.json"
    metrics_path = tmp_path / "metrics.md"
    baseline_path = tmp_path / "baseline.json"
    bundle_path.write_text(json.dumps(bundle), encoding="utf-8")
    metrics_path.write_text(metrics, encoding="utf-8")
    baseline_path.write_text(
        json.dumps(baseline if baseline is not None else _baseline()),
        encoding="utf-8",
    )
    command = [
        sys.executable,
        str(CHECKER),
        "--bundle",
        str(bundle_path),
        "--metrics",
        str(metrics_path),
        "--baseline",
        str(baseline_path),
    ]
    if as_json:
        command.append("--json")
    return subprocess.run(
        command,
        capture_output=True,
        text=True,
        check=False,
    )


def test_passing_fixture_prints_artifact_paths(tmp_path: Path) -> None:
    result = _run(tmp_path, _bundle(tmp_path))
    assert result.returncode == 0
    assert "std/certs/core.proof.json" in result.stdout


def test_trusted_atom_requires_matching_baseline(tmp_path: Path) -> None:
    bundle = _bundle(tmp_path)
    matching = _run(
        tmp_path / "matching",
        bundle,
        baseline=_baseline(trusted={"std/core": ["trusted"]}),
    )
    assert matching.returncode == 0

    missing = _run(
        tmp_path / "missing",
        bundle,
        baseline=_baseline(include_trusted=False),
        as_json=True,
    )
    assert missing.returncode == 1
    result = json.loads(missing.stdout)
    assert "baseline_mismatch: trusted" in "\n".join(result["failures"])
    assert "update scripts/std_proof_baseline.json" in "\n".join(result["failures"])
    assert result["observed"]["modules"] == {}
    assert result["observed"]["trusted"] == {"std/core": ["trusted"]}


def test_missing_trusted_baseline_key_means_empty(tmp_path: Path) -> None:
    bundle = _bundle(tmp_path)
    bundle["modules"]["std/core"]["atoms"] = [
        atom
        for atom in bundle["modules"]["std/core"]["atoms"]
        if atom["status"] != "trusted"
    ]
    bundle["summary"].update(total_atoms=2, proven_atoms=2, trusted_atoms=0)
    metrics = METRICS.replace(
        "Atoms total:** 3 (2 proven · 1 trusted)",
        "Atoms total:** 2 (2 proven · 0 trusted)",
    )
    baseline = {
        "total_modules": 2,
        "total_atoms": 2,
        "proven_atoms": 2,
        "lean_verified_atoms": 0,
        "trusted_atoms": 0,
        "modules": {},
    }
    result = _run(
        tmp_path,
        bundle,
        metrics,
        baseline=baseline,
    )
    assert result.returncode == 0


def test_trusted_suggestions_match_packaged_certificate_checker(
    tmp_path: Path,
) -> None:
    bundle = _bundle(tmp_path)
    drift = _run(
        tmp_path / "drift",
        bundle,
        baseline=_baseline(include_trusted=False),
        as_json=True,
    )
    drift_observed = json.loads(drift.stdout)["observed"]

    root = tmp_path / "package"
    certs_dir = root / "std" / "certs"
    certs_dir.mkdir(parents=True)
    for module, certificate in bundle["modules"].items():
        relative = Path(module.removeprefix("std/"))
        cert_path = certs_dir / relative.with_suffix(".proof.json")
        cert_path.parent.mkdir(parents=True, exist_ok=True)
        cert_path.write_text(
            json.dumps({"atoms": certificate["atoms"]}), encoding="utf-8"
        )
        source_path = root / "std" / relative.with_suffix(".mm")
        source_path.parent.mkdir(parents=True, exist_ok=True)
        source_path.write_text("atom fixture {}\n", encoding="utf-8")

    stub = tmp_path / "mumei"
    stub.write_text(
        "#!/usr/bin/env python3\nprint('All verified: true')\n",
        encoding="utf-8",
    )
    stub.chmod(stub.stat().st_mode | stat.S_IXUSR)
    baseline_path = tmp_path / "package-baseline.json"
    baseline_path.write_text(
        json.dumps(_baseline(include_trusted=False)), encoding="utf-8"
    )
    packaged = subprocess.run(
        [
            sys.executable,
            str(PACKAGED_CHECKER),
            str(root),
            "--mumei-bin",
            str(stub),
            "--baseline",
            str(baseline_path),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert packaged.returncode == 1
    marker = "update scripts/std_proof_baseline.json with:\n"
    packaged_suggestion = json.loads(packaged.stderr.split(marker, 1)[1])
    assert packaged_suggestion["trusted"] == drift_observed["trusted"]
    assert packaged_suggestion["modules"] == drift_observed["modules"]


def test_module_missing_from_bundle(tmp_path: Path) -> None:
    bundle = _bundle(tmp_path)
    del bundle["modules"]["std/io"]
    result = _run(tmp_path, bundle)
    assert result.returncode == 1
    assert "module_missing" in result.stderr


def test_trusted_count_mismatch(tmp_path: Path) -> None:
    bundle = _bundle(tmp_path)
    bundle["summary"]["trusted_atoms"] = 0
    result = _run(tmp_path, bundle)
    assert result.returncode == 1
    assert "trusted_count_mismatch" in result.stderr


def test_atom_count_mismatch(tmp_path: Path) -> None:
    bundle = _bundle(tmp_path)
    bundle["summary"]["total_atoms"] = 4
    result = _run(tmp_path, bundle)
    assert result.returncode == 1
    assert "atom_count_mismatch" in result.stderr


def test_baseline_improvement_requires_update(tmp_path: Path) -> None:
    bundle = _bundle(tmp_path)
    bundle["summary"]["proven_atoms"] = 3
    bundle["modules"]["std/core"]["atoms"][1]["z3_check_result"] = "unsat"
    baseline_path = tmp_path / "baseline.json"
    baseline_path.write_text(
        json.dumps(
            {
                "total_modules": 2,
                "total_atoms": 3,
                "proven_atoms": 2,
                "lean_verified_atoms": 0,
                "trusted_atoms": 1,
                "modules": {},
            }
        ),
        encoding="utf-8",
    )
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
            "--baseline",
            str(baseline_path),
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 1
    assert "baseline_mismatch" in result.stderr
    assert "update scripts/std_proof_baseline.json" in result.stderr
    assert '"proven_atoms": 3' in result.stderr


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
            "--baseline",
            str(tmp_path / "baseline.json"),
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
