from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
CHECKER = ROOT / "scripts" / "verify_packaged_certs.py"


def _stub_binary(tmp_path: Path) -> Path:
    stub = tmp_path / "mumei"
    stub.write_text(
        """#!/usr/bin/env python3
import json
import sys
from pathlib import Path

cert = json.loads(Path(sys.argv[2]).read_text())
if cert.get("stub_result", "pass") == "fail":
    print("Results: 0 proven, 0 changed, 1 unproven, 0 missing")
    sys.exit(1)
print("All verified: true")
""",
        encoding="utf-8",
    )
    stub.chmod(stub.stat().st_mode | stat.S_IXUSR)
    return stub


def _write_case(
    tmp_path: Path,
    *,
    atoms: list[dict[str, str]],
    stub_result: str = "pass",
    source: bool = True,
    baseline_atoms: list[dict[str, str]] | None = None,
) -> tuple[Path, Path, Path]:
    root = tmp_path / "package"
    cert_dir = root / "std" / "certs"
    cert_dir.mkdir(parents=True)
    cert = {
        "atoms": atoms,
        "stub_result": stub_result,
    }
    (cert_dir / "example.proof.json").write_text(
        json.dumps(cert), encoding="utf-8"
    )
    if source:
        (root / "std" / "example.mm").write_text("atom example {}\n", encoding="utf-8")
    baseline = root / "baseline.json"
    baseline.write_text(
        json.dumps(
            {
                "total_modules": 1,
                "total_atoms": len(atoms),
                "proven_atoms": sum(
                    atom["z3_check_result"] in {"unsat", "lean_verified"}
                    for atom in atoms
                ),
                "lean_verified_atoms": sum(
                    atom["z3_check_result"] == "lean_verified" for atom in atoms
                ),
                "trusted_atoms": sum(
                    atom.get("status") == "trusted" for atom in atoms
                ),
                "modules": (
                    {"std/example": baseline_atoms}
                    if baseline_atoms is not None
                    else {}
                ),
            }
        ),
        encoding="utf-8",
    )
    return root, _stub_binary(tmp_path), baseline


def _run(root: Path, binary: Path, baseline: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            str(CHECKER),
            str(root),
            "--mumei-bin",
            str(binary),
            "--baseline",
            str(baseline),
        ],
        capture_output=True,
        text=True,
        check=False,
        env=os.environ.copy(),
    )


def test_clean_pass(tmp_path: Path) -> None:
    root, binary, baseline = _write_case(
        tmp_path,
        atoms=[{"name": "ok", "status": "verified", "z3_check_result": "unsat"}],
    )
    result = _run(root, binary, baseline)
    assert result.returncode == 0
    assert "checked=1 failed=0" in result.stdout


def test_baseline_tolerated_failure(tmp_path: Path) -> None:
    non_proven = [{"name": "unknown", "z3_check_result": "unknown"}]
    root, binary, baseline = _write_case(
        tmp_path,
        atoms=[
            {
                "name": "unknown",
                "status": "failed",
                "z3_check_result": "unknown",
            }
        ],
        stub_result="fail",
        baseline_atoms=non_proven,
    )
    result = _run(root, binary, baseline)
    assert result.returncode == 0


def test_new_unproven_atom_fails(tmp_path: Path) -> None:
    root, binary, baseline = _write_case(
        tmp_path,
        atoms=[
            {
                "name": "new",
                "status": "failed",
                "z3_check_result": "unknown",
            }
        ],
        stub_result="fail",
    )
    result = _run(root, binary, baseline)
    assert result.returncode == 1
    assert "new non-proven atoms" in result.stderr


def test_baseline_module_improved_fails_with_update(tmp_path: Path) -> None:
    root, binary, baseline = _write_case(
        tmp_path,
        atoms=[{"name": "fixed", "status": "verified", "z3_check_result": "unsat"}],
        baseline_atoms=[{"name": "fixed", "z3_check_result": "unknown"}],
    )
    result = _run(root, binary, baseline)
    assert result.returncode == 1
    assert "update scripts/std_proof_baseline.json" in result.stderr
    assert '"modules": {}' in result.stderr


def test_missing_source_fails(tmp_path: Path) -> None:
    root, binary, baseline = _write_case(
        tmp_path,
        atoms=[{"name": "ok", "status": "verified", "z3_check_result": "unsat"}],
        source=False,
    )
    result = _run(root, binary, baseline)
    assert result.returncode == 1
    assert "missing source" in result.stderr
