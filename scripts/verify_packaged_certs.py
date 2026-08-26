#!/usr/bin/env python3
"""Verify packaged std/ certificates against the committed std baseline.

The verifier accepts the known, certificate-derived exceptions recorded in
``scripts/std_proof_baseline.json``.  A certificate that fails for any other
reason, gains or loses a non-proven atom, or has a hash/integrity failure is a
hard failure.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

PROVEN_RESULTS = {"unsat", "lean_verified"}
INTEGRITY_MARKERS = (
    "certificate_hash mismatch",
    "certificate_hash absent",
    "certificate was modified",
    "no longer matches",
    "tamper",
)
CHANGED_RE = re.compile(r"\b[1-9]\d*\s+changed\b")
MISSING_RE = re.compile(r"\b[1-9]\d*\s+missing\b")


def _module_key(cert: Path, certs_dir: Path) -> str:
    relative = cert.relative_to(certs_dir)
    stem = relative.with_suffix("")
    if stem.suffix == ".proof":
        stem = stem.with_suffix("")
    return f"std/{stem.as_posix()}"


def _non_proven_atoms(cert: dict) -> list[dict[str, str]]:
    atoms = cert.get("atoms", [])
    if not isinstance(atoms, list):
        raise ValueError("certificate atoms must be a list")
    pairs = {
        (str(atom.get("name", "")), str(atom.get("z3_check_result", "")))
        for atom in atoms
        if atom.get("z3_check_result") not in PROVEN_RESULTS
    }
    return [
        {"name": name, "z3_check_result": result}
        for name, result in sorted(pairs)
    ]


def _baseline_modules(baseline: dict) -> dict[str, list[dict[str, str]]]:
    modules = baseline.get("modules", {})
    if not isinstance(modules, dict):
        raise ValueError("baseline modules must be an object")
    normalized: dict[str, list[dict[str, str]]] = {}
    for module, atoms in modules.items():
        if not isinstance(module, str) or not isinstance(atoms, list):
            raise ValueError("baseline module entries must be atom lists")
        normalized[module] = sorted(
            [
                {
                    "name": str(atom["name"]),
                    "z3_check_result": str(atom["z3_check_result"]),
                }
                for atom in atoms
            ],
            key=lambda item: (item["name"], item["z3_check_result"]),
        )
    return normalized


def _has_integrity_failure(output: str) -> bool:
    lowered = output.lower()
    return (
        any(marker in lowered for marker in INTEGRITY_MARKERS)
        or CHANGED_RE.search(lowered) is not None
        or MISSING_RE.search(lowered) is not None
    )


def _is_expected_unproven_failure(output: str) -> bool:
    return "results:" in output.lower() and "unproven" in output.lower()


def _format_baseline_update(observed: dict) -> str:
    return json.dumps(observed, indent=2, ensure_ascii=False)


def verify(
    root: Path,
    mumei_bin: Path,
    baseline_path: Path,
) -> tuple[int, int, list[str]]:
    root = root.resolve()
    certs_dir = root / "std" / "certs"
    baseline = json.loads(baseline_path.read_text(encoding="utf-8"))
    baseline_modules = _baseline_modules(baseline)
    certs = sorted(certs_dir.rglob("*.proof.json")) if certs_dir.is_dir() else []
    failures: list[str] = []
    observed_modules: set[str] = set()
    observed_non_proven: dict[str, list[dict[str, str]]] = {}
    total_atoms = 0
    proven_atoms = 0
    lean_verified_atoms = 0
    trusted_atoms = 0
    improved_modules: set[str] = set()

    for cert_path in certs:
        module = _module_key(cert_path, certs_dir)
        observed_modules.add(module)
        source = root / "std" / f"{cert_path.relative_to(certs_dir)}"
        source = source.with_suffix("").with_suffix(".mm")
        try:
            certificate = json.loads(cert_path.read_text(encoding="utf-8"))
            non_proven = _non_proven_atoms(certificate)
            atoms = certificate["atoms"]
            total_atoms += len(atoms)
            proven_atoms += sum(
                atom.get("z3_check_result") in PROVEN_RESULTS for atom in atoms
            )
            lean_verified_atoms += sum(
                atom.get("z3_check_result") == "lean_verified" for atom in atoms
            )
            trusted_atoms += sum(
                atom.get("status") == "trusted" for atom in atoms
            )
            if non_proven:
                observed_non_proven[module] = non_proven
        except (OSError, json.JSONDecodeError, TypeError, ValueError) as exc:
            failures.append(f"{module}: invalid certificate: {exc}")
            continue

        if not source.is_file():
            failures.append(
                f"{module}: missing source {source} for certificate {cert_path}"
            )
            continue

        result = subprocess.run(
            [str(mumei_bin), "verify-cert", str(cert_path), str(source), "--strict"],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode == 0:
            if module in baseline_modules:
                improved_modules.add(module)
            continue

        output = f"{result.stdout}\n{result.stderr}"
        expected = baseline_modules.get(module)
        if (
            expected is not None
            and expected == non_proven
            and not _has_integrity_failure(output)
            and _is_expected_unproven_failure(output)
        ):
            continue
        if expected is None:
            reason = "new non-proven atoms"
        elif expected != non_proven:
            reason = "changed non-proven atoms"
        else:
            reason = "certificate integrity failure"
        failures.append(
            f"{module}: {reason}; observed non-proven atoms:\n"
            f"{json.dumps(non_proven, indent=2, ensure_ascii=False)}"
        )

    missing_baseline = sorted(set(baseline_modules) - observed_modules)
    for module in missing_baseline:
        failures.append(
            f"{module}: baseline module has no packaged certificate"
        )

    if improved_modules:
        observed_baseline = {
            "total_modules": len(certs),
            "total_atoms": total_atoms,
            "proven_atoms": proven_atoms,
            "lean_verified_atoms": lean_verified_atoms,
            "trusted_atoms": trusted_atoms,
            "modules": observed_non_proven,
        }
        for module in sorted(improved_modules):
            failures.append(
                f"{module}: certificate now passes cleanly; update "
                "scripts/std_proof_baseline.json with:\n"
                f"{_format_baseline_update(observed_baseline)}"
            )

    return len(certs), len(failures), failures


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("root", nargs="?", type=Path, help="root containing std/")
    parser.add_argument("--root", dest="root_option", type=Path)
    parser.add_argument("--mumei-bin", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, required=True)
    args = parser.parse_args(argv)
    root = args.root_option or args.root
    if root is None:
        parser.error("a root directory containing std/ is required")

    try:
        checked, failed, failures = verify(
            root,
            args.mumei_bin,
            args.baseline,
        )
    except (OSError, json.JSONDecodeError, TypeError, ValueError) as exc:
        print(f"verify packaged certificates: input error: {exc}", file=sys.stderr)
        return 1

    for failure in failures:
        print(f"::error::{failure}", file=sys.stderr)
    print(f"verify packaged certificates: checked={checked} failed={failed}")
    if checked == 0:
        print("::error::no certificates were checked", file=sys.stderr)
        return 1
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
