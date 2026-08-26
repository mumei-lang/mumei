"""Bundle all std/ proof certificates into a single distributable JSON.

Part of SI-5 Phase 3-C (Cross-Project Proof Certificate Sharing). The
GitHub Actions workflow ``generate-std-certs.yml`` produces per-module
``.proof.json`` artefacts under ``std/certs/``; this script fans them back
in to a single ``std-proof-bundle.json`` that is distributed via
``homebrew-mumei`` so downstream projects can verify imports against a
trusted, mumei-versioned certificate set.

Bundle schema::

    {
      "bundle_version": "1.1",
      "generated_at": "2026-04-18T...",
      "mumei_version": "<string>",
      "modules": {
          "std/core": { ...ProofCertificate... },
          "std/prelude": { ...ProofCertificate... },
          ...
      },
      "artifact_paths": [
          "std/certs/core.proof.json",
          ...
      ],
      "lean_provenance": [
          {
              "module": "std/core",
              "atom": "complex_atom",
              "z3_check_result": "lean_verified",
              "translator_version": "...",
              "bridge_lemma_hash": "...",
              "manual_lemma_reason": null
          }
      ],
      "summary": {
          "total_modules": 17,
          "all_verified": 15,
          "partial_verified": 2,
          "total_atoms": 150,
          "proven_atoms": 140,
          "lean_verified_atoms": 2,
          "trusted_atoms": 0
      }
    }

Usage::

    python scripts/bundle_std_certs.py \\
        [--certs-dir std/certs] \\
        [--output std-proof-bundle.json] \\
        [--mumei-version 0.4.2]
"""
from __future__ import annotations

import argparse
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

BUNDLE_VERSION = "1.1"
REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CERTS_DIR = REPO_ROOT / "std" / "certs"
DEFAULT_OUTPUT = REPO_ROOT / "std-proof-bundle.json"


def _module_key(cert_path: Path, certs_root: Path) -> str:
    """Map ``std/certs/foo/bar.proof.json`` -> ``std/foo/bar`` bundle key."""
    rel = cert_path.relative_to(certs_root)
    stem = rel.with_suffix("")  # drop .json
    if stem.suffix == ".proof":
        stem = stem.with_suffix("")
    return f"std/{stem.as_posix()}"


def _summarize(modules: dict) -> dict:
    total_modules = len(modules)
    all_verified = 0
    partial_verified = 0
    total_atoms = 0
    proven_atoms = 0
    lean_verified_atoms = 0
    trusted_atoms = 0

    for cert in modules.values():
        results = cert.get("atoms", cert.get("results", []))
        module_atoms = len(results)
        module_proven = sum(
            1 for r in results if r.get("status") == "unsat"
        )
        lean_verified_atoms += sum(
            1 for atom in cert.get("atoms", [])
            if atom.get("z3_check_result") == "lean_verified"
        )
        trusted_atoms += sum(1 for r in results if r.get("status") == "trusted")
        total_atoms += module_atoms
        proven_atoms += module_proven
        if module_atoms == 0:
            # No atoms recorded — treat as fully verified (e.g. effect-only).
            all_verified += 1
        elif module_proven == module_atoms:
            all_verified += 1
        else:
            partial_verified += 1

    return {
        "total_modules": total_modules,
        "all_verified": all_verified,
        "partial_verified": partial_verified,
        "total_atoms": total_atoms,
        "proven_atoms": proven_atoms,
        "lean_verified_atoms": lean_verified_atoms,
        "trusted_atoms": trusted_atoms,
    }


def _artifact_path(cert_path: Path, certs_dir: Path) -> str:
    """Return a deterministic repo-relative certificate path when possible."""
    try:
        return cert_path.resolve().relative_to(REPO_ROOT).as_posix()
    except ValueError:
        return (
            Path("std/certs")
            .joinpath(cert_path.resolve().relative_to(certs_dir.resolve()))
            .as_posix()
        )


def _lean_provenance(modules: dict) -> list[dict]:
    provenance: list[dict] = []
    for module, cert in modules.items():
        for atom in cert.get("atoms", []):
            if atom.get("z3_check_result") != "lean_verified":
                continue
            provenance.append(
                {
                    "module": module,
                    "atom": atom.get("name", ""),
                    "z3_check_result": "lean_verified",
                    "translator_version": atom.get("translator_version", ""),
                    "bridge_lemma_hash": atom.get("bridge_lemma_hash", ""),
                    "manual_lemma_reason": atom.get("manual_lemma_reason"),
                }
            )
    return provenance


def build_bundle(certs_dir: Path, mumei_version: str) -> dict:
    """Collect every ``*.proof.json`` under ``certs_dir`` into a bundle dict."""
    if not certs_dir.exists():
        raise FileNotFoundError(f"certs dir not found: {certs_dir}")

    modules: dict = {}
    artifact_paths: list[str] = []
    for cert_path in sorted(certs_dir.rglob("*.proof.json")):
        try:
            cert = json.loads(cert_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            print(
                f"warning: failed to read {cert_path}: {exc}",
                file=sys.stderr,
            )
            continue
        key = _module_key(cert_path, certs_dir)
        modules[key] = cert
        artifact_paths.append(_artifact_path(cert_path, certs_dir))
    return {
        "bundle_version": BUNDLE_VERSION,
        "generated_at": datetime.now(timezone.utc).isoformat(
            timespec="seconds",
        ),
        "mumei_version": mumei_version,
        "modules": modules,
        "artifact_paths": artifact_paths,
        "lean_provenance": _lean_provenance(modules),
        "summary": _summarize(modules),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--certs-dir",
        type=Path,
        default=DEFAULT_CERTS_DIR,
        help="Directory containing per-module *.proof.json files.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_OUTPUT,
        help="Bundle output path (default: std-proof-bundle.json).",
    )
    parser.add_argument(
        "--mumei-version",
        default="unknown",
        help="mumei version string embedded in the bundle metadata.",
    )
    parser.add_argument(
        "--stdout",
        action="store_true",
        help="Print the bundle JSON to stdout instead of writing a file.",
    )
    args = parser.parse_args(argv)

    bundle = build_bundle(args.certs_dir, args.mumei_version)
    payload = json.dumps(bundle, indent=2, ensure_ascii=False)

    if args.stdout:
        sys.stdout.write(payload + "\n")
        return 0

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(payload + "\n", encoding="utf-8")
    summary = bundle["summary"]
    print(
        f"wrote {args.output} ({summary['total_modules']} modules, "
        f"{summary['proven_atoms']}/{summary['total_atoms']} atoms proven)",
    )
    return 0


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
