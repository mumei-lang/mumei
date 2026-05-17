"""Tests for the enhanced list_std_catalog() function in mcp_server.py."""
from __future__ import annotations

import json
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

# Add project root to path so we can import mcp_server
sys.path.insert(0, str(Path(__file__).parent.parent))


def _get_catalog() -> dict:
    """Call list_std_catalog() and parse the JSON result."""
    # Import inline to avoid MCP server startup side effects
    from mcp_server import list_std_catalog
    raw = list_std_catalog()
    return json.loads(raw)


class TestListStdCatalog:
    """Tests for the enhanced list_std_catalog() function."""

    def test_discovers_all_std_modules(self) -> None:
        """All .mm files under std/ are discovered."""
        catalog = _get_catalog()
        assert "modules" in catalog
        modules = catalog["modules"]
        assert len(modules) > 0

        # Verify known modules exist
        paths = {m["path"] for m in modules}
        assert "std/contracts.mm" in paths
        assert "std/prelude.mm" in paths

    def test_contracts_has_expected_types(self) -> None:
        """std/contracts.mm has the expected refinement types."""
        catalog = _get_catalog()
        contracts = next(
            m for m in catalog["modules"] if m["path"] == "std/contracts.mm"
        )
        type_names = [t.split("=")[0].strip() for t in contracts["types"]]
        assert "type Port" in type_names
        assert "type NonNegative" in type_names

    def test_contracts_has_expected_atoms(self) -> None:
        """std/contracts.mm has atoms with requires/ensures fields."""
        catalog = _get_catalog()
        contracts = next(
            m for m in catalog["modules"] if m["path"] == "std/contracts.mm"
        )
        atoms = contracts["atoms"]
        assert len(atoms) > 0

        # Each atom should be a dict with signature, requires, ensures
        for atom in atoms:
            assert "signature" in atom
            assert "requires" in atom
            assert "ensures" in atom
            assert "effects" in atom

        # Check a specific atom
        clamp = next(
            (a for a in atoms if "clamp" in a["signature"]), None
        )
        assert clamp is not None
        assert clamp["requires"] != ""
        assert clamp["ensures"] != ""

    def test_effects_mm_has_all_effect_forms(self) -> None:
        """std/effects.mm captures non-parameterized, parameterized, and composite effects."""
        catalog = _get_catalog()
        effects_mod = next(
            (m for m in catalog["modules"] if m["path"] == "std/effects.mm"),
            None,
        )
        assert effects_mod is not None, "std/effects.mm not found in catalog"

        effect_names = [e.split("(")[0].split(" includes")[0].strip() for e in effects_mod["effects"]]

        # Non-parameterized effects
        assert "effect FileRead" in effect_names
        assert "effect FileWrite" in effect_names
        assert "effect Network" in effect_names
        assert "effect Log" in effect_names
        assert "effect Console" in effect_names

        # Parameterized effects
        assert "effect HttpGet" in effect_names
        assert "effect HttpPost" in effect_names

        # Composite effects
        composite = [e for e in effects_mod["effects"] if "includes:" in e]
        assert len(composite) >= 3, f"Expected >=3 composite effects, got {composite}"

    def test_http_server_stateful_effect_has_transitions(self) -> None:
        """std/http_server.mm stateful effect includes all transition definitions."""
        catalog = _get_catalog()
        http_mod = next(
            (m for m in catalog["modules"] if m["path"] == "std/http_server.mm"),
            None,
        )
        assert http_mod is not None, "std/http_server.mm not found in catalog"
        assert len(http_mod["effects"]) >= 1

        # The HttpServer effect should be a single joined string
        http_server_effect = http_mod["effects"][0]
        assert "effect HttpServer" in http_server_effect
        assert "states:" in http_server_effect
        assert "initial: Init" in http_server_effect

        # All 5 transitions must be present
        assert "transition bind: Init -> Bound" in http_server_effect
        assert "transition listen: Bound -> Listening" in http_server_effect
        assert "transition accept: Listening -> Responding" in http_server_effect
        assert "transition respond: Responding -> Listening" in http_server_effect
        assert "transition close: Listening -> Init" in http_server_effect

    def test_http_secure_has_effects(self) -> None:
        """std/http_secure.mm has effect definitions."""
        catalog = _get_catalog()
        modules_by_path = {m["path"]: m for m in catalog["modules"]}

        # Find http_secure module if it exists
        http_mod = modules_by_path.get("std/http_secure.mm")
        if http_mod is None:
            pytest.skip("std/http_secure.mm not found")

        # Should have effects list
        assert "effects" in http_mod

    def test_module_has_description(self) -> None:
        """Modules with leading comments have a description field."""
        catalog = _get_catalog()
        contracts = next(
            m for m in catalog["modules"] if m["path"] == "std/contracts.mm"
        )
        assert "description" in contracts
        # contracts.mm has a leading comment block
        assert contracts["description"] != ""

    def test_module_entry_has_all_fields(self) -> None:
        """Each module entry has the expected fields."""
        catalog = _get_catalog()
        for module in catalog["modules"]:
            assert "path" in module
            assert "import" in module
            assert "description" in module
            assert "types" in module
            assert "atoms" in module
            assert "structs" in module
            assert "effects" in module


class _FakeRunningProcess:
    def __init__(self) -> None:
        self.terminated = False

    def poll(self) -> int | None:
        return 0 if self.terminated else None

    def terminate(self) -> None:
        self.terminated = True


class _FakeCompletedProcess:
    returncode = 0

    def communicate(self, timeout: float | None = None) -> tuple[str, str]:
        return "", ""

    def poll(self) -> int:
        return self.returncode


class TestOrchestrationWorkerPool:
    def test_cancel_task_terminates_bound_process(self) -> None:
        from mcp_server import Z3WorkerPool

        pool = Z3WorkerPool(max_workers=1, timeout_ms=100, memory_limit_mb=64)
        worker = pool.acquire_worker()
        process = _FakeRunningProcess()
        worker.process = process
        pool.bind_task("task-1", worker.worker_id)

        assert pool.cancel_task("task-1") is True
        assert process.terminated is True

        pool.release_worker(worker.worker_id)


class TestOrchestrationCacheIsolation:
    def test_fingerprint_changes_with_solver_features(self) -> None:
        from mcp_server import _compute_mcp_solver_config_fingerprint

        base = _compute_mcp_solver_config_fingerprint(30000, 1024)

        assert base != _compute_mcp_solver_config_fingerprint(10000, 1024)
        assert base != _compute_mcp_solver_config_fingerprint(30000, 2048)
        assert base != _compute_mcp_solver_config_fingerprint(
            30000,
            1024,
            has_string_constraints=True,
        )
        assert base != _compute_mcp_solver_config_fingerprint(
            30000,
            1024,
            has_array_forall=True,
        )

    def test_detects_source_features_for_cache_keys(self) -> None:
        from mcp_server import _detect_mcp_solver_features

        source = "atom f(s: Str, arr: [i64]) forall(i, 0, n, arr[i] > 0);"

        assert _detect_mcp_solver_features(source) == {
            "has_string_constraints": True,
            "has_array_forall": True,
        }


class TestVerifyWithOrchestration:
    def test_invocation_writes_certificate_and_env_metadata(self) -> None:
        from mcp_server import verify_with_orchestration, _z3_worker_pool

        popen_calls = []

        def fake_popen(args: list[str], **kwargs) -> _FakeCompletedProcess:
            popen_calls.append((args, kwargs))
            output_path = Path(args[args.index("--output") + 1])
            output_path.write_text(json.dumps({"atoms": []}), encoding="utf-8")
            report_dir = Path(args[args.index("--report-dir") + 1])
            (report_dir / "report.json").write_text(
                json.dumps({"status": "ok"}),
                encoding="utf-8",
            )
            return _FakeCompletedProcess()

        with patch("mcp_server.subprocess.Popen", side_effect=fake_popen):
            raw = verify_with_orchestration(
                "atom f() ensures: result == 0; body: 0;",
                timeout_ms=1234,
                enable_cache=False,
            )

        _z3_worker_pool.shutdown()
        payload = json.loads(raw)
        args, kwargs = popen_calls[0]
        env = kwargs["env"]

        assert payload["status"] == "passed"
        assert payload["proof_certificate"] == {"atoms": []}
        assert "--proof-cert" in args
        assert env["MUMEI_TASK_ID"] == payload["task_id"]
        assert env["MUMEI_SOLVER_CACHE_KEY"] == payload["cache_key"]
        assert env["MUMEI_VERIFICATION_TIMEOUT_MS"] == "1234"


class TestOrchestrationResumeValidation:
    """Tests for orchestration task resume safeguards."""

    def test_missing_resume_task_returns_error(self) -> None:
        """Unknown task IDs are rejected instead of starting unrelated work."""
        from mcp_server import _resume_task_validation_error

        error = _resume_task_validation_error("missing-task", None, "source-a", "cache-a")

        assert error is not None
        assert error["status"] == "error"
        assert error["task_id"] == "missing-task"
        assert error["error"] == "task_id not found"

    def test_mismatched_resume_task_returns_error(self) -> None:
        """Task IDs cannot be resumed with a different source or solver config."""
        from mcp_server import VerificationTask, _resume_task_validation_error

        task = VerificationTask(
            task_id="task-1",
            source_hash="source-a",
            cache_key="cache-a",
        )
        error = _resume_task_validation_error("task-1", task, "source-b", "cache-b")

        assert error is not None
        assert error["status"] == "error"
        assert error["task_id"] == "task-1"
        assert error["expected_source_hash"] == "source-a"
        assert error["requested_source_hash"] == "source-b"
        assert error["expected_cache_key"] == "cache-a"
        assert error["requested_cache_key"] == "cache-b"

    def test_matching_resume_task_has_no_error(self) -> None:
        """Matching source and solver cache keys are safe to resume."""
        from mcp_server import VerificationTask, _resume_task_validation_error

        task = VerificationTask(
            task_id="task-1",
            source_hash="source-a",
            cache_key="cache-a",
        )

        assert _resume_task_validation_error("task-1", task, "source-a", "cache-a") is None
