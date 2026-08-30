#!/usr/bin/env python3
"""Record the trust surface of large-scale verification cases.

For every ``.mm`` case the script

1. verifies it and emits a proof certificate for **every** atom;
2. re-checks that certificate with ``mumei verify-cert --strict``;
3. records the trust surface: application-side ``trusted atom`` count, FFI
   boundary count (``extern`` blocks and their declarations), and the number of
   atoms whose Z3 result was not ``unsat`` and therefore escalate to Lean;
4. records Z3 wall-clock solver time and the ``budget_policy_fingerprint``
   carried by the certificate.

``std/`` is measured with the same counters used by
``scripts/generate_stdlib_metrics.py`` so the "0 trusted atoms in ``std/``"
invariant is checked against the very same definition while scaling up.

The emitted JSON is a measurement artifact: it reuses the canonical
``budget_policy_fingerprint`` / ``lean_verified`` spellings and introduces no
audit or verdict vocabulary.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO_ROOT / "scripts"))

from generate_stdlib_metrics import _count_metrics  # noqa: E402

ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
ATOM_RE = re.compile(r"^\s*(?:trusted\s+|async\s+)?atom\s+\w+")
TRUSTED_ATOM_RE = re.compile(r"^\s*trusted\s+atom\s+\w+")
EXTERN_BLOCK_RE = re.compile(r"^\s*extern\s+\"")
VERIFIED_RE = re.compile(r"(\d+) item\(s\) verified")
ESCALATION_RE = re.compile(r"(\d+) Lean escalation candidate\(s\)")


def strip_ansi(text: str) -> str:
    return ANSI.sub("", text)


def source_counts(path: Path) -> dict[str, int]:
    atoms = trusted = extern_blocks = extern_decls = 0
    in_extern = False
    depth = 0
    for line in path.read_text(encoding="utf-8").splitlines():
        if ATOM_RE.match(line):
            atoms += 1
        if TRUSTED_ATOM_RE.match(line):
            trusted += 1
        if EXTERN_BLOCK_RE.match(line):
            extern_blocks += 1
            in_extern = True
            depth = 0
        if in_extern:
            depth += line.count("{") - line.count("}")
            if re.match(r"^\s*(fn|atom)\s+\w+", line):
                extern_decls += 1
            if depth <= 0 and "{" in line and "}" in line:
                in_extern = False
            elif depth <= 0 and extern_blocks and "}" in line:
                in_extern = False
    return {
        "atoms": atoms,
        "trusted_atoms": trusted,
        "ffi_extern_blocks": extern_blocks,
        "ffi_boundary_declarations": extern_decls,
    }


def std_trusted_atoms(std_dir: Path) -> dict[str, int]:
    atoms = trusted = 0
    for path in sorted(std_dir.rglob("*.mm")):
        file_atoms, file_trusted, _ = _count_metrics(path)
        atoms += file_atoms
        trusted += file_trusted
    return {"std_atoms": atoms, "std_trusted_atoms": trusted}


def measure_case(
    binary: Path,
    source: Path,
    cert_dir: Path,
    fingerprint: str | None,
    timeout: int,
) -> dict[str, object]:
    cert_path = cert_dir / f"{source.parent.name}.proof-cert.json"
    env = dict(os.environ)
    if fingerprint:
        env["MUMEI_BUDGET_POLICY_FINGERPRINT"] = fingerprint

    started = time.perf_counter()
    verify = subprocess.run(
        [str(binary), "verify", str(source), "--proof-cert", "--output", str(cert_path)],
        cwd=cert_dir,
        capture_output=True,
        text=True,
        timeout=timeout,
        env=env,
    )
    solver_seconds = round(time.perf_counter() - started, 3)
    output = strip_ansi(verify.stdout + verify.stderr)
    verified_match = VERIFIED_RE.search(output)
    escalation_match = ESCALATION_RE.search(output)

    strict = subprocess.run(
        [str(binary), "verify-cert", "--strict", str(cert_path), str(source)],
        cwd=cert_dir,
        capture_output=True,
        text=True,
        timeout=timeout,
        env=env,
    )

    cert = json.loads(cert_path.read_text(encoding="utf-8")) if cert_path.exists() else {}
    cert_atoms = cert.get("atoms", [])
    unknown = [
        atom["name"] for atom in cert_atoms if atom.get("z3_check_result") not in {"unsat", None}
    ]

    counts = source_counts(source)
    return {
        "case": source.parent.name,
        "source": f"{source.parent.name}/{source.name}",
        "atom_count": counts["atoms"],
        "proof_certificate": cert_path.name,
        "certified_atoms": len(cert_atoms),
        "all_atoms_certified": len(cert_atoms) == counts["atoms"],
        "verify_exit_code": verify.returncode,
        "verified_items": int(verified_match.group(1)) if verified_match else 0,
        "verify_cert_strict_exit_code": strict.returncode,
        "verify_cert_strict": strict.returncode == 0,
        "trust_surface": {
            "application_trusted_atoms": counts["trusted_atoms"],
            "ffi_extern_blocks": counts["ffi_extern_blocks"],
            "ffi_boundary_declarations": counts["ffi_boundary_declarations"],
            "z3_unknown_to_lean_escalation_atoms": len(unknown),
            "lean_escalation_candidates": int(escalation_match.group(1))
            if escalation_match
            else 0,
        },
        "z3_solver_seconds": solver_seconds,
        "budget_policy_fingerprint": cert.get("budget_policy_fingerprint"),
        "lean_verified": bool(cert.get("lean_verified", False)),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("sources", nargs="+", type=Path, help="Large-scale .mm sources.")
    parser.add_argument("--mumei-bin", type=Path, default=REPO_ROOT / "target" / "release" / "mumei")
    parser.add_argument("--cert-dir", type=Path, required=True, help="Directory for certificates.")
    parser.add_argument("--std-dir", type=Path, default=REPO_ROOT / "std")
    parser.add_argument("--budget-policy-fingerprint", default=None)
    parser.add_argument("--timeout", type=int, default=1800)
    parser.add_argument("--output", type=Path, default=None)
    args = parser.parse_args(argv)

    binary = args.mumei_bin.resolve()
    if not binary.exists():
        fallback = REPO_ROOT / "target" / "debug" / "mumei"
        if not fallback.exists():
            print(f"mumei binary not found: {binary}", file=sys.stderr)
            return 2
        binary = fallback

    cert_dir = args.cert_dir.resolve()
    cert_dir.mkdir(parents=True, exist_ok=True)

    cases = [
        measure_case(
            binary,
            source.resolve(),
            cert_dir,
            args.budget_policy_fingerprint,
            args.timeout,
        )
        for source in args.sources
    ]
    std_counts = std_trusted_atoms(args.std_dir.resolve())

    report = {
        "schema": "mumei.scale_trust_surface/v1",
        "mumei_binary": binary.name,
        "budget_policy_fingerprint": args.budget_policy_fingerprint,
        "std_trust_surface": std_counts,
        "cases": cases,
        "totals": {
            "atom_count": sum(int(case["atom_count"]) for case in cases),
            "certified_atoms": sum(int(case["certified_atoms"]) for case in cases),
            "application_trusted_atoms": sum(
                int(case["trust_surface"]["application_trusted_atoms"]) for case in cases
            ),
            "ffi_boundary_declarations": sum(
                int(case["trust_surface"]["ffi_boundary_declarations"]) for case in cases
            ),
            "z3_unknown_to_lean_escalation_atoms": sum(
                int(case["trust_surface"]["z3_unknown_to_lean_escalation_atoms"])
                for case in cases
            ),
            "z3_solver_seconds": round(sum(float(case["z3_solver_seconds"]) for case in cases), 3),
            "verify_cert_strict_passed": sum(1 for case in cases if case["verify_cert_strict"]),
        },
    }

    text = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output is None:
        sys.stdout.write(text)
    else:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text, encoding="utf-8")
        print(f"scale trust surface written to {args.output}")

    failures = [case["case"] for case in cases if not case["verify_cert_strict"]]
    if failures:
        print(f"verify-cert --strict failed: {', '.join(failures)}", file=sys.stderr)
        return 1
    if std_counts["std_trusted_atoms"] != 0:
        print(
            f"std/ trusted atom count regressed to {std_counts['std_trusted_atoms']}",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
