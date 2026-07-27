"""Regression guard: the std/ library must contain 0 trusted atoms.

The trusted-atom budget was reduced to 0 on ``develop`` (see
``docs/TRUSTED_ATOMS.md`` and ``docs/STDLIB_METRICS.md``). This guard fails
CI if a new (or edited) std module reintroduces a ``trusted atom`` declaration,
so the "no trusted atoms remain in std/" contract cannot silently regress.

It scans the live ``std/`` tree with the same counter used by
``scripts/generate_stdlib_metrics.py`` and cross-checks the checked-in
metrics/inventory docs so a doc edit alone cannot mask a real regression.
"""
from __future__ import annotations

import importlib.util
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
STD_DIR = REPO_ROOT / "std"
METRICS_DOC = REPO_ROOT / "docs" / "STDLIB_METRICS.md"
TRUSTED_DOC = REPO_ROOT / "docs" / "TRUSTED_ATOMS.md"


def _load_metrics_module():
    spec = importlib.util.spec_from_file_location(
        "generate_stdlib_metrics",
        REPO_ROOT / "scripts" / "generate_stdlib_metrics.py",
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


gsm = _load_metrics_module()


def test_no_trusted_atoms_in_std() -> None:
    """Every std/*.mm module must declare 0 trusted atoms."""
    rows = gsm.analyze_metrics(STD_DIR, mumei_bin=None)
    assert rows, "expected to scan at least one std module"
    offenders = {r["path"]: r["trusted"] for r in rows if r["trusted"] > 0}
    assert not offenders, (
        "std/ must contain 0 trusted atoms, but found trusted declarations in: "
        f"{offenders}. If a new module needs a trusted atom, strengthen the "
        "contract or escalate to mumei-lean instead of reintroducing trust."
    )
    assert sum(r["trusted"] for r in rows) == 0


def test_metrics_doc_reports_zero_trusted() -> None:
    """docs/STDLIB_METRICS.md summary must still report 0 trusted atoms."""
    text = METRICS_DOC.read_text(encoding="utf-8")
    summary = re.search(
        r"Trusted atoms \(reviewed contracts\):\*\*\s*(\d+)", text
    )
    assert summary is not None, "trusted-atom summary line missing from metrics doc"
    assert summary.group(1) == "0", (
        "docs/STDLIB_METRICS.md reports a non-zero trusted-atom count"
    )
    atoms_total = re.search(
        r"Atoms total:\*\*\s*\d+\s*\(\d+\s*proven\s*[·\u00b7]\s*(\d+)\s*trusted\)",
        text,
    )
    assert atoms_total is not None and atoms_total.group(1) == "0"


def test_metrics_doc_matches_live_scan_trusted_total() -> None:
    """The doc's trusted total must equal the live-scanned trusted total."""
    rows = gsm.analyze_metrics(STD_DIR, mumei_bin=None)
    live_trusted = sum(r["trusted"] for r in rows)
    text = METRICS_DOC.read_text(encoding="utf-8")
    summary = re.search(
        r"Trusted atoms \(reviewed contracts\):\*\*\s*(\d+)", text
    )
    assert summary is not None
    assert int(summary.group(1)) == live_trusted


def test_trusted_inventory_doc_declares_zero() -> None:
    """docs/TRUSTED_ATOMS.md must still declare 0 trusted atoms."""
    text = TRUSTED_DOC.read_text(encoding="utf-8")
    assert "0 trusted atoms" in text
    assert "No trusted atoms remain in `std/`" in text
