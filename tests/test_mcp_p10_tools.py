"""Tests for the P10 cross-repo MCP tools.

These tools (``measure_std_health``, ``get_proof_certificate``,
``generate_doc``) live in ``mcp_server.py`` and let external MCP
clients (Claude Code / Devin / Codex) work with proof health and
proof certificates without having to run the agent locally.

The tests favor hermeticity over end-to-end coverage:

- ``measure_std_health`` is exercised against the real ``std/``
  directory but with the underlying ``subprocess.run`` patched so we
  do not depend on a working ``mumei`` binary in CI.
- ``get_proof_certificate`` is tested with synthetic bundles / cert
  files written under ``tmp_path`` (and ``Path(__file__).parent`` is
  monkey-patched as needed).
- ``generate_doc`` is exercised with ``subprocess.run`` patched.
"""
from __future__ import annotations

import json
import sys
import subprocess
import types
from pathlib import Path
from unittest.mock import patch, MagicMock

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent))

import mcp_server  # noqa: E402


def _payload(raw: str) -> dict:
    assert isinstance(raw, str), f"expected str, got {type(raw)}"
    return json.loads(raw)


# ---------------------------------------------------------------------------
# _compute_health_score
# ---------------------------------------------------------------------------


class TestComputeHealthScore:
    def test_zero_atoms_returns_zero(self) -> None:
        assert mcp_server._compute_health_score(0, 0, 0, 0) == 0.0

    def test_all_verified_no_trusted_no_todos(self) -> None:
        assert mcp_server._compute_health_score(10, 10, 0, 0) == 1.0

    def test_trusted_reduces_score(self) -> None:
        assert mcp_server._compute_health_score(10, 10, 5, 0) == 0.5

    def test_todo_penalty(self) -> None:
        # 10 atoms verified, no trusted, 5 TODOs → 1.0 - 0.05 = 0.95
        assert mcp_server._compute_health_score(10, 10, 0, 5) == 0.95

    def test_clamped_to_zero(self) -> None:
        # negative base + huge penalty → 0.0
        assert mcp_server._compute_health_score(10, 0, 0, 1000) == 0.0

    def test_clamped_to_one(self) -> None:
        # absurd inputs that would produce >1 stay clamped
        assert mcp_server._compute_health_score(1, 100, 0, 0) == 1.0


# ---------------------------------------------------------------------------
# measure_std_health
# ---------------------------------------------------------------------------


class TestMeasureStdHealth:
    def test_returns_expected_shape(self) -> None:
        # Force every subprocess invocation to "succeed" so the tool can
        # walk std/ end-to-end without needing a real mumei binary.
        with patch(
            "mcp_server.subprocess.run",
            return_value=subprocess.CompletedProcess(
                args=[], returncode=0, stdout="", stderr=""
            ),
        ):
            payload = _payload(mcp_server.measure_std_health())
        assert {
            "total_files",
            "verified_files",
            "failed_files",
            "total_atoms",
            "verified_atoms",
            "trusted_atoms",
            "todo_count",
            "health_score",
            "details",
        } <= payload.keys()
        assert payload["total_files"] >= 1
        assert isinstance(payload["details"], list)

    def test_failures_are_counted(self) -> None:
        # Every verify call returns failure → all files end up failed.
        with patch(
            "mcp_server.subprocess.run",
            return_value=subprocess.CompletedProcess(
                args=[], returncode=1, stdout="", stderr="boom"
            ),
        ):
            payload = _payload(mcp_server.measure_std_health())
        assert payload["verified_files"] == 0
        assert payload["failed_files"] == payload["total_files"]

    def test_handles_missing_binary(self) -> None:
        with patch(
            "mcp_server.subprocess.run", side_effect=FileNotFoundError("mumei")
        ):
            payload = _payload(mcp_server.measure_std_health())
        # Every file should be marked verify_unavailable.
        statuses = {d.get("status") for d in payload["details"]}
        assert "verify_unavailable" in statuses
        assert payload["verified_files"] == 0


# ---------------------------------------------------------------------------
# get_proof_certificate
# ---------------------------------------------------------------------------


class TestGetProofCertificate:
    def test_rejects_path_traversal(self) -> None:
        payload = _payload(mcp_server.get_proof_certificate("../etc/passwd"))
        assert "error" in payload
        assert "repo-relative" in payload["error"]

    def test_rejects_empty(self) -> None:
        payload = _payload(mcp_server.get_proof_certificate(""))
        assert "error" in payload

    def test_returns_error_when_missing(self, tmp_path: Path, monkeypatch) -> None:
        payload = _payload(
            mcp_server.get_proof_certificate("std/__definitely_missing__")
        )
        assert "error" in payload
        assert payload["module"] == "std/__definitely_missing__"

    def test_reads_cert_file(self, tmp_path: Path, monkeypatch) -> None:
        # Stage a fake certificate under <repo>/std/certs/<name>.proof.json.
        repo_root = Path(mcp_server.__file__).parent
        certs_dir = repo_root / "std" / "certs"
        certs_dir.mkdir(parents=True, exist_ok=True)
        cert_path = certs_dir / "__p10_test_mod.proof.json"
        cert_path.write_text(
            json.dumps({"status": "verified", "atoms": ["foo"]}),
            encoding="utf-8",
        )
        try:
            payload = _payload(
                mcp_server.get_proof_certificate("std/__p10_test_mod")
            )
            assert payload["status"] == "verified"
            assert payload["atoms"] == ["foo"]
        finally:
            cert_path.unlink(missing_ok=True)

    def test_reads_bundle(self, monkeypatch) -> None:
        # Stage a fake std-proof-bundle.json at the repo root.
        repo_root = Path(mcp_server.__file__).parent
        bundle_path = repo_root / "std-proof-bundle.json"
        bundle_path.write_text(
            json.dumps(
                {
                    "bundle_version": "1.0",
                    "mumei_version": "test-version",
                    "modules": {
                        "std/__p10_bundle_mod": {
                            "results": [],
                            "atoms": ["bar"],
                        }
                    },
                }
            ),
            encoding="utf-8",
        )
        try:
            payload = _payload(
                mcp_server.get_proof_certificate("std/__p10_bundle_mod")
            )
            assert payload["source"] == "std-proof-bundle.json"
            assert payload["certificate"]["atoms"] == ["bar"]
            assert payload["bundle_version"] == "1.0"
        finally:
            bundle_path.unlink(missing_ok=True)


# ---------------------------------------------------------------------------
# generate_doc
# ---------------------------------------------------------------------------


class TestGenerateDoc:
    def test_empty_source_returns_error(self) -> None:
        payload = _payload(mcp_server.generate_doc(""))
        assert "error" in payload

    def test_invalid_format_returns_error(self) -> None:
        payload = _payload(
            mcp_server.generate_doc("atom x() ensures: true; body: 0;", format="pdf")
        )
        assert "error" in payload

    def test_json_format_returns_parsed(self) -> None:
        fake_doc = {"atoms": [{"name": "x"}]}
        completed = subprocess.CompletedProcess(
            args=[],
            returncode=0,
            stdout=json.dumps(fake_doc),
            stderr="",
        )
        with patch("mcp_server.subprocess.run", return_value=completed):
            payload = _payload(
                mcp_server.generate_doc(
                    "atom x() ensures: true; body: 0;",
                    format="json",
                )
            )
        assert payload["format"] == "json"
        assert payload["doc"] == fake_doc

    def test_doc_failure_returns_error(self) -> None:
        completed = subprocess.CompletedProcess(
            args=[], returncode=1, stdout="", stderr="parse error"
        )
        with patch("mcp_server.subprocess.run", return_value=completed):
            payload = _payload(
                mcp_server.generate_doc(
                    "garbage source", format="json"
                )
            )
        assert "error" in payload
        assert "parse error" in payload["stderr"]

    def test_markdown_returns_stdout(self) -> None:
        completed = subprocess.CompletedProcess(
            args=[], returncode=0, stdout="# Doc\n", stderr=""
        )
        with patch("mcp_server.subprocess.run", return_value=completed):
            payload = _payload(
                mcp_server.generate_doc(
                    "atom x() ensures: true; body: 0;",
                    format="markdown",
                )
            )
        assert payload["format"] == "markdown"
        assert payload["stdout"].startswith("# Doc")
