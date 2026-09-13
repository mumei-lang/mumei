"""CI regression gate for the P27 six-axis evaluation suite.

Mirrors ``tests/test_benchmark_suite.py``: the suite must measure all six
`PAPER_DRAFT.md` §7 axes deterministically over the controlled benchmark task
set, and must degrade to ``SKIP`` — never to a substituted value — when an
input is absent.
"""
from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
SUITE_SCRIPT = REPO_ROOT / "benchmarks" / "evaluation_suite.py"


def load_suite():
    spec = importlib.util.spec_from_file_location("evaluation_suite", SUITE_SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


@pytest.fixture(scope="module")
def suite():
    return load_suite()


def test_axes_match_paper_evaluation_dimensions(suite):
    assert suite.AXES == (
        "proof_success_rate",
        "repair_convergence",
        "counterexample_quality",
        "trust_surface",
        "user_burden",
        "runtime_artifact_utility",
    )


def test_status_vocabulary_is_the_three_fixed_axis_statuses(suite):
    # MEASURED / SKIP are the run_benchmarks statuses; INCOMPLETE is the
    # suite-level partial-coverage status. Any further status is a schema change.
    assert suite.STATUS_MEASURED == "MEASURED"
    assert suite.STATUS_SKIP == "SKIP"
    assert suite.STATUS_INCOMPLETE == "INCOMPLETE"
    assert {
        name for name in dir(suite) if name.startswith("STATUS_")
    } == {"STATUS_MEASURED", "STATUS_SKIP", "STATUS_INCOMPLETE"}
    assert suite.SCHEMA == "mumei.evaluation_suite/v1"


def test_controlled_task_set_is_the_existing_benchmark_categories(suite):
    harness = suite.load_run_benchmarks()
    assert set(harness.CATEGORIES) == {
        "arithmetic",
        "concurrency",
        "dafny_puzzles",
        "domain_compliance",
        "state_machine",
        "svcomp_style",
    }
    for dir_path in harness.CATEGORIES.values():
        assert dir_path.is_dir()
        assert sorted(dir_path.glob("*.mm"))


# ---------------------------------------------------------------------------
# Axis 5: user burden
# ---------------------------------------------------------------------------


SAMPLE_SOURCE = """
atom transfer {
    requires: amount > 0;
    ensures: result == amount;
    effect_pre: balance >= amount;
    effect_post: balance == old(balance) - amount;
    body: {
        return amount;
    }
}

trusted atom clock {
    ensures: true;
}
"""


def test_user_burden_counts_clause_kinds_and_tokens(suite, tmp_path):
    source = tmp_path / "sample.mm"
    source.write_text(SAMPLE_SOURCE, encoding="utf-8")

    burden = suite.measure_user_burden(source)

    assert burden["atoms"] == 2
    assert burden["spec_clause_kinds"] == {
        "requires": 1,
        "ensures": 2,
        "invariant": 0,
        "effect_pre": 1,
        "effect_post": 1,
    }
    assert burden["spec_clauses"] == 5
    assert burden["spec_clauses_per_atom"] == 2.5
    assert burden["impl_tokens"] > 0
    assert burden["spec_tokens"] > burden["impl_tokens"]
    assert burden["spec_to_impl_token_ratio"] == round(
        burden["spec_tokens"] / burden["impl_tokens"], 4
    )


NESTED_INVARIANT_SOURCE = """
atom sum_array(arr: [i64], n: i64)
requires: n >= 0;
ensures: result >= 0;
body: {
    let sum = 0;
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n && sum >= 0
    decreases: n - i
    {
        sum = sum + arr[i];
        i = i + 1;
    };
    sum
};
"""


def test_user_burden_counts_invariant_clauses_inside_a_body(suite, tmp_path):
    source = tmp_path / "loop.mm"
    source.write_text(NESTED_INVARIANT_SOURCE, encoding="utf-8")

    burden = suite.measure_user_burden(source)

    assert burden["spec_clause_kinds"]["invariant"] == 1
    assert burden["spec_clauses"] == 3

    stripped = tmp_path / "loop_without_invariant.mm"
    stripped.write_text(
        "\n".join(
            line
            for line in NESTED_INVARIANT_SOURCE.splitlines()
            if "invariant:" not in line
        ),
        encoding="utf-8",
    )
    baseline = suite.measure_user_burden(stripped)
    invariant_tokens = len(suite.TOKEN_RE.findall(" i >= 0 && i <= n && sum >= 0"))
    # the clause counts as specification and leaves the implementation untouched
    assert burden["spec_tokens"] == baseline["spec_tokens"] + invariant_tokens
    assert burden["impl_tokens"] == baseline["impl_tokens"]


def test_committed_benchmarks_report_their_loop_invariants(suite):
    harness = suite.load_run_benchmarks()
    source = harness.CATEGORIES["svcomp_style"] / "loop_invariant.mm"
    assert suite.measure_user_burden(source)["spec_clause_kinds"]["invariant"] == 1


def test_user_burden_is_deterministic(suite, tmp_path):
    source = tmp_path / "sample.mm"
    source.write_text(SAMPLE_SOURCE, encoding="utf-8")
    assert suite.measure_user_burden(source) == suite.measure_user_burden(source)


def test_user_burden_ratio_is_skipped_without_implementation(suite, tmp_path):
    source = tmp_path / "trusted_only.mm"
    source.write_text(
        "trusted atom clock {\n    ensures: true;\n}\n", encoding="utf-8"
    )
    burden = suite.measure_user_burden(source)
    assert burden["impl_tokens"] == 0
    assert burden["spec_to_impl_token_ratio"] is None


def test_user_burden_aggregate_is_atom_weighted(suite, tmp_path):
    small = tmp_path / "small.mm"
    small.write_text(SAMPLE_SOURCE, encoding="utf-8")
    files = [suite.measure_user_burden(small), suite.measure_user_burden(small)]
    aggregate = suite.aggregate_user_burden(files)
    assert aggregate["status"] == "MEASURED"
    assert aggregate["atoms"] == 4
    assert aggregate["spec_clauses"] == 10
    assert aggregate["spec_clauses_per_atom"] == 2.5


def test_user_burden_skips_without_sources(suite):
    assert suite.aggregate_user_burden([])["status"] == "SKIP"


# ---------------------------------------------------------------------------
# Axis 2: repair convergence
# ---------------------------------------------------------------------------


def _certificate(file_name: str, summary: dict) -> dict:
    return {"file": f"benchmarks/arithmetic/{file_name}", "self_correction_summary": summary}


def test_repair_convergence_aggregates_self_correction_summary(suite, tmp_path):
    cert_dir = tmp_path / "certs"
    cert_dir.mkdir()
    (cert_dir / "a.json").write_text(
        json.dumps(
            _certificate(
                "a.mm",
                {
                    "total_atoms": 4,
                    "converged_atoms": 3,
                    "convergence_rate": 0.75,
                    "average_repair_attempts": 2.0,
                    "total_token_cost": 400,
                },
            )
        ),
        encoding="utf-8",
    )
    (cert_dir / "b.json").write_text(
        json.dumps(
            _certificate(
                "b.mm",
                {
                    "total_atoms": 1,
                    "converged_atoms": 1,
                    "convergence_rate": 1.0,
                    "average_repair_attempts": 7.0,
                    "total_token_cost": 100,
                },
            )
        ),
        encoding="utf-8",
    )

    summaries = suite.load_self_correction_summaries(cert_dir)
    assert set(summaries) == {
        "a.mm",
        "b.mm",
        "arithmetic/a.mm",
        "arithmetic/b.mm",
    }

    repair = suite.aggregate_repair_convergence(
        ["arithmetic/a.mm", "arithmetic/b.mm"], summaries
    )
    assert repair["status"] == "MEASURED"
    assert repair["total_atoms"] == 5
    assert repair["converged_atoms"] == 4
    assert repair["convergence_rate"] == 0.8
    # atom-weighted, matching SelfCorrectionSummary::from_atom_metadata
    assert repair["average_repair_attempts"] == 3.0
    assert repair["total_token_cost"] == 500


def test_repair_convergence_distinguishes_same_name_across_categories(suite, tmp_path):
    cert_dir = tmp_path / "certs"
    cert_dir.mkdir()
    for category, atoms in (("arithmetic", 4), ("concurrency", 1)):
        (cert_dir / f"{category}.json").write_text(
            json.dumps({
                "file": f"benchmarks/{category}/shared.mm",
                "self_correction_summary": {
                    "total_atoms": atoms,
                    "converged_atoms": atoms,
                    "convergence_rate": 1.0,
                    "average_repair_attempts": 1.0,
                    "total_token_cost": 10 * atoms,
                },
            }),
            encoding="utf-8",
        )

    summaries = suite.load_self_correction_summaries(cert_dir)
    # the ambiguous bare name is dropped; only qualified lookups resolve
    assert "shared.mm" not in summaries
    assert suite.aggregate_repair_convergence(["shared.mm"], summaries)[
        "status"
    ] == "SKIP"
    assert (
        suite.aggregate_repair_convergence(["concurrency/shared.mm"], summaries)[
            "total_atoms"
        ]
        == 1
    )


def test_repair_convergence_skips_without_data(suite, tmp_path):
    assert suite.load_self_correction_summaries(None) == {}
    assert suite.load_self_correction_summaries(tmp_path / "missing") == {}
    repair = suite.aggregate_repair_convergence(["a.mm"], {})
    assert repair["status"] == "SKIP"
    assert repair["files_with_repair_data"] == 0


def test_repair_convergence_ignores_certificates_without_summary(suite, tmp_path):
    cert_dir = tmp_path / "certs"
    cert_dir.mkdir()
    (cert_dir / "plain.json").write_text(
        json.dumps({"file": "benchmarks/arithmetic/a.mm", "lean_verified": False}),
        encoding="utf-8",
    )
    (cert_dir / "broken.json").write_text("{not json", encoding="utf-8")
    assert suite.load_self_correction_summaries(cert_dir) == {}


# ---------------------------------------------------------------------------
# Axis 6: runtime artifact utility
# ---------------------------------------------------------------------------


def _artifact_file(name: str, emitted: dict[str, bool]) -> dict:
    targets = {
        target: {"emitted": emitted[target], "artifacts": 1 if emitted[target] else 0}
        for target in ("llvm-ir", "c-header", "verified-json", "proof-cert")
    }
    return {
        "file": name,
        "targets": targets,
        "emitted_targets": sum(1 for t in targets.values() if t["emitted"]),
    }


def test_runtime_artifact_targets_cover_paper_artifacts(suite):
    assert suite.ARTIFACT_TARGETS == (
        "llvm-ir",
        "c-header",
        "verified-json",
        "proof-cert",
    )


def test_runtime_artifact_aggregation_counts_per_target(suite):
    files = [
        _artifact_file(
            "ok.mm",
            {
                "llvm-ir": True,
                "c-header": True,
                "verified-json": True,
                "proof-cert": True,
            },
        ),
        _artifact_file(
            "partial.mm",
            {
                "llvm-ir": False,
                "c-header": True,
                "verified-json": True,
                "proof-cert": True,
            },
        ),
    ]
    artifacts = suite.aggregate_runtime_artifacts(files)
    assert artifacts["status"] == "MEASURED"
    assert artifacts["attempted_emissions"] == 8
    assert artifacts["successful_emissions"] == 7
    assert artifacts["emission_success_rate"] == 0.875
    assert artifacts["per_target_success"] == {
        "llvm-ir": 1,
        "c-header": 2,
        "verified-json": 2,
        "proof-cert": 2,
    }


def test_runtime_artifact_skips_without_measurements(suite):
    assert suite.aggregate_runtime_artifacts([])["status"] == "SKIP"


def test_expected_fail_tasks_demand_only_the_refutation_certificate(suite):
    assert suite._expected_emission("PASS", "llvm-ir") is True
    assert suite._expected_emission("PASS", "proof-cert") is True
    assert suite._expected_emission("FAIL", "proof-cert") is True
    for target in suite.BUILD_EMIT_TARGETS:
        assert suite._expected_emission("FAIL", target) is False


def test_runtime_artifact_counterexample_tasks_measured_separately(suite):
    all_on = {t: True for t in suite.ARTIFACT_TARGETS}
    ok = _artifact_file("ok.mm", all_on)
    ok["expected"] = "PASS"
    refused = _artifact_file(
        "bad_fail.mm",
        {"llvm-ir": False, "c-header": False, "verified-json": False, "proof-cert": True},
    )
    refused["expected"] = "FAIL"
    leaked = _artifact_file(
        "leak_fail.mm",
        {"llvm-ir": True, "c-header": False, "verified-json": False, "proof-cert": False},
    )
    leaked["expected"] = "FAIL"

    artifacts = suite.aggregate_runtime_artifacts([ok, refused, leaked])
    # the P27 PASS-side figures are unaffected by counterexample tasks
    assert artifacts["attempted_emissions"] == 4
    assert artifacts["successful_emissions"] == 4
    assert artifacts["emission_success_rate"] == 1.0
    ce = artifacts["counterexample"]
    assert ce["files"] == 2
    assert ce["attempted_emissions"] == 8
    assert ce["as_expected_emissions"] == 4 + 2
    assert ce["as_expected_rate"] == 0.75
    assert ce["leaked_build_artifacts"] == 1
    assert ce["refutation_certificates"] == 1


def test_runtime_artifact_leak_counts_partial_output_of_rejected_build(suite):
    # a rejected build (exit 1) that still left a non-empty artifact behind is a leak
    partial = _artifact_file(
        "partial_fail.mm",
        {"llvm-ir": False, "c-header": False, "verified-json": False, "proof-cert": True},
    )
    partial["expected"] = "FAIL"
    partial["targets"]["llvm-ir"]["artifacts"] = 1

    ce = suite.aggregate_runtime_artifacts([partial])["counterexample"]
    assert ce["leaked_build_artifacts"] == 1
    assert ce["as_expected_emissions"] == 3
    assert suite._as_expected("FAIL", "llvm-ir", partial["targets"]["llvm-ir"]) is False
    assert suite._as_expected("FAIL", "proof-cert", partial["targets"]["proof-cert"]) is True


def test_runtime_artifact_proof_cert_escalates_an_inconclusive_verify(
    suite, monkeypatch, tmp_path
):
    """`verify --proof-cert` exit 3 is not a verdict: with a bridge the
    `--escalate-lean` re-run decides, without one no certificate is credited."""
    source = tmp_path / "ff.mm"
    source.write_text("atom a { ensures: true; }", encoding="utf-8")
    calls: list[list[str]] = []
    envs: list[dict | None] = []

    class _Proc:
        def __init__(self, code: int) -> None:
            self.returncode = code
            self.stdout = self.stderr = ""

    def _run(cmd, **kwargs):
        calls.append(cmd)
        envs.append(kwargs.get("env"))
        if cmd[1] == "build":
            return _Proc(1)
        out = Path(cmd[cmd.index("--output") + 1])
        out.write_text("{}", encoding="utf-8")
        return _Proc(suite.EXIT_VERIFIED if "--escalate-lean" in cmd else suite.EXIT_INCONCLUSIVE)

    monkeypatch.setattr(suite.subprocess, "run", _run)

    with_bridge = suite.measure_runtime_artifacts(
        "mumei", source, tmp_path, expected="PASS", lean_bridge=Path("bridge.py")
    )
    escalated = [(c, e) for c, e in zip(calls, envs) if "--escalate-lean" in c]
    assert len(escalated) == 1
    # the CLI resolves the bridge from its cwd (a temp dir), so it must be told
    assert escalated[0][1]["MUMEI_LEAN_PATH"] == "bridge.py"
    assert with_bridge["targets"]["proof-cert"] == {
        "emitted": True,
        "artifacts": 1,
        "as_expected": True,
    }

    calls.clear()
    alone = suite.measure_runtime_artifacts("mumei", source, tmp_path, expected="PASS")
    assert not [c for c in calls if "--escalate-lean" in c]
    assert alone["targets"]["proof-cert"]["emitted"] is False
    assert alone["targets"]["proof-cert"]["artifacts"] == 0


# ---------------------------------------------------------------------------
# B-7: AI Lean proof path on/off
# ---------------------------------------------------------------------------


def _ai_cert(file: str, atoms: list[dict]) -> dict:
    return {"file": file, "atoms": atoms}


def _lean_atom(name: str, result: str, *, ai_used: bool = False, attempts: int | None = None,
               reason: str | None = None, solver_s: float = 1.0) -> dict:
    meta: dict = {"lean_solver_time_s": solver_s, "ai_proof_used": ai_used}
    if attempts is not None:
        meta["ai_proof_attempts"] = attempts
    atom: dict = {"name": name, "z3_check_result": result, "lean_metadata": meta}
    if reason:
        atom["manual_lemma_reason"] = reason
    return atom


def test_load_ai_proof_certificates_keys_by_category_and_file(suite, tmp_path):
    cert_dir = tmp_path / "on"
    (cert_dir / "arithmetic").mkdir(parents=True)
    (cert_dir / "arithmetic" / "a.proof.json").write_text(
        json.dumps(_ai_cert("benchmarks/arithmetic/a.mm", [])), encoding="utf-8"
    )
    (cert_dir / "run.json").write_text(json.dumps({"schema": "x", "files": []}), encoding="utf-8")
    (cert_dir / "broken.json").write_text("{", encoding="utf-8")
    assert list(suite.load_ai_proof_certificates(cert_dir)) == ["arithmetic/a.mm"]
    assert suite.load_ai_proof_certificates(None) == {}


def test_load_ai_proof_certificates_honours_run_manifest_allow_list(suite, tmp_path):
    cert_dir = tmp_path / "on"
    (cert_dir / "arithmetic").mkdir(parents=True)
    listed = cert_dir / "arithmetic" / "a.proof.json"
    listed.write_text(json.dumps(_ai_cert("benchmarks/arithmetic/a.mm", [])), encoding="utf-8")
    # stale certificate-shaped JSON sitting next to the measured run
    (cert_dir / "arithmetic" / "stale.proof.json").write_text(
        json.dumps(_ai_cert("benchmarks/arithmetic/stale.mm", [])), encoding="utf-8"
    )
    manifest = {
        "schema": suite.AI_PROOF_RUN_MANIFEST_SCHEMA,
        "ai_proof": "on",
        "files": [{"file": "arithmetic/a.mm", "certificate": str(listed)}],
    }
    (cert_dir / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    assert list(suite.load_ai_proof_certificates(cert_dir)) == ["arithmetic/a.mm"]

    # relative certificate paths resolve against the manifest directory
    manifest["files"][0]["certificate"] = "arithmetic/a.proof.json"
    (cert_dir / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    assert list(suite.load_ai_proof_certificates(cert_dir)) == ["arithmetic/a.mm"]

    # an AI-off manifest vouches for nothing: no certificate is an AI-on one
    manifest["ai_proof"] = "off"
    (cert_dir / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    assert suite.load_ai_proof_certificates(cert_dir) == {}


def test_summarize_ai_proof_certificate_reads_bridge_provenance_only(suite):
    harness = suite.load_run_benchmarks()
    cert = _ai_cert(
        "benchmarks/arithmetic/a.mm",
        [
            _lean_atom("p", "lean_verified", ai_used=True, attempts=2, reason="nonlinear"),
            _lean_atom("q", "lean_verified", ai_used=False, solver_s=2.5),
            # a rejected/unknown atom never counts as AI-used even if flagged
            _lean_atom("r", "unknown", ai_used=True, attempts=3, reason="nonlinear"),
        ],
    )
    summary = suite.summarize_ai_proof_certificate(cert, harness)
    assert summary["lean_verified_atoms"] == 2
    assert summary["ai_proof_used_atoms"] == 1
    assert summary["ai_proof_attempts"] == 5
    assert summary["manual_lemma_reason_remaining"] == 1
    assert summary["lean_solver_time_s"] == 2.5


def _category_result(details: list[dict]) -> dict:
    return {"details": details}


def _detail(
    file: str,
    candidates: int,
    verified: int,
    solver_s: float | None,
    left: int | None,
    *,
    lean_status: str | None = None,
    hashes: dict[str, str] | None = None,
) -> dict:
    if lean_status is None:
        lean_status = "MEASURED" if solver_s is not None else "SKIP"
    return {
        "file": file,
        "escalation_candidates": candidates,
        "lean_verified_atoms": verified,
        "lean_solver_time_s": solver_s,
        "lean_status": lean_status,
        "manual_lemma_reason_remaining": left,
        "atom_content_hashes": hashes,
    }


def test_aggregate_lean_ai_proof_reports_delta_between_off_and_on(suite):
    harness = suite.load_run_benchmarks()
    result = _category_result(
        [
            _detail("plain.mm", 0, 0, None, None),
            _detail("hard.mm", 2, 1, 4.0, 1),
        ]
    )
    certs = {
        "arithmetic/hard.mm": _ai_cert(
            "benchmarks/arithmetic/hard.mm",
            [
                _lean_atom("a", "lean_verified"),
                _lean_atom("b", "lean_verified", ai_used=True, attempts=2, reason="nonlinear"),
            ],
        )
    }
    block = suite.aggregate_lean_ai_proof("arithmetic", result, certs, harness)
    assert block["off"] == {
        "status": "MEASURED",
        "unmeasured_files": [],
        "files": 1,
        "lean_verified_atoms": 1,
        "manual_lemma_reason_remaining": 1,
        "lean_solver_time_s": 4.0,
    }
    assert block["on"]["status"] == "MEASURED"
    assert block["on"]["lean_verified_atoms"] == 2
    assert block["on"]["ai_proof_used_atoms"] == 1
    assert block["on"]["ai_proof_attempts"] == 2
    assert block["on"]["manual_lemma_reason_remaining"] == 0
    assert block["on"]["file_names"] == ["hard.mm"]
    assert block["lean_verified_delta"] == 1


def test_aggregate_lean_ai_proof_skips_without_binary_or_certificates(suite):
    harness = suite.load_run_benchmarks()
    result = _category_result([_detail("hard.mm", 2, 0, None, None)])
    block = suite.aggregate_lean_ai_proof("arithmetic", result, {}, harness, binary_available=False)
    assert block == {"off": {"status": "SKIP"}, "on": {"status": "SKIP", "files": 0}, "lean_verified_delta": None}
    # candidates present but no Lean sample (bridge unavailable) is not a measurement
    block = suite.aggregate_lean_ai_proof("arithmetic", result, {}, harness)
    assert block["off"]["status"] == "SKIP"
    assert block["lean_verified_delta"] is None


def test_aggregate_lean_ai_proof_partial_coverage_is_incomplete_without_delta(suite):
    harness = suite.load_run_benchmarks()
    result = _category_result(
        [
            _detail("a.mm", 2, 2, 3.0, 0),
            _detail("b.mm", 2, 2, 3.0, 0),
        ]
    )
    certs = {
        "arithmetic/a.mm": _ai_cert(
            "benchmarks/arithmetic/a.mm",
            [_lean_atom("x", "lean_verified"), _lean_atom("y", "lean_verified")],
        )
    }
    block = suite.aggregate_lean_ai_proof("arithmetic", result, certs, harness)
    assert block["off"]["lean_verified_atoms"] == 4
    assert block["on"]["status"] == suite.STATUS_INCOMPLETE
    assert block["on"]["files"] == 1
    assert block["on"]["missing_files"] == ["b.mm"]
    assert block["lean_verified_delta"] is None
    # an incomplete category contributes to neither side of the suite totals
    totals = suite._lean_ai_proof_totals([block])
    assert totals["off"] == {"status": "SKIP"}
    assert totals["on"] == {"status": "SKIP"}
    assert totals["lean_verified_delta"] is None
    assert totals["unpaired_categories"] == 1
    assert suite.STATUS_INCOMPLETE in suite._fmt_lean_ai_on(block)


def test_aggregate_lean_ai_proof_failed_or_timed_out_off_run_has_no_delta(suite):
    harness = suite.load_run_benchmarks()
    certs = {
        "arithmetic/a.mm": _ai_cert(
            "benchmarks/arithmetic/a.mm",
            [_lean_atom("x", "lean_verified"), _lean_atom("y", "lean_verified")],
        ),
        "arithmetic/b.mm": _ai_cert("benchmarks/arithmetic/b.mm", [_lean_atom("z", "lean_verified")]),
    }
    for status in ("TIMEOUT", "FAIL"):
        result = _category_result(
            [
                _detail("a.mm", 2, 0, 300.0, None, lean_status=status),
                _detail("b.mm", 1, 1, 2.0, 0),
            ]
        )
        block = suite.aggregate_lean_ai_proof("arithmetic", result, certs, harness)
        assert block["off"]["status"] == suite.STATUS_INCOMPLETE
        assert block["off"]["unmeasured_files"] == [["a.mm", status]]
        assert block["on"]["status"] == "MEASURED"
        assert block["lean_verified_delta"] is None
        assert suite._lean_ai_proof_totals([block])["lean_verified_delta"] is None


def test_aggregate_lean_ai_proof_stale_on_certificate_is_incomplete(suite):
    harness = suite.load_run_benchmarks()
    on_atoms = [_lean_atom("x", "lean_verified"), _lean_atom("y", "lean_verified")]
    for i, atom in enumerate(on_atoms):
        atom["content_hash"] = f"h{i}"
    certs = {"arithmetic/a.mm": _ai_cert("benchmarks/arithmetic/a.mm", on_atoms)}

    same = _detail("a.mm", 2, 2, 3.0, 0, hashes={"x": "h0", "y": "h1"})
    block = suite.aggregate_lean_ai_proof("arithmetic", _category_result([same]), certs, harness)
    assert block["on"]["status"] == "MEASURED"
    assert block["on"]["stale_files"] == []
    assert block["lean_verified_delta"] == 0

    # source changed since the on run: one atom left, other content
    changed = _detail("a.mm", 1, 1, 3.0, 0, hashes={"x": "h9"})
    block = suite.aggregate_lean_ai_proof("arithmetic", _category_result([changed]), certs, harness)
    assert block["on"]["status"] == suite.STATUS_INCOMPLETE
    assert block["on"]["stale_files"] == ["a.mm"]
    assert block["lean_verified_delta"] is None


def test_load_self_correction_summaries_honours_repair_run_manifest(suite, tmp_path):
    cert_dir = tmp_path / "repair"
    (cert_dir / "arithmetic").mkdir(parents=True)
    summary = {"total_atoms": 1, "converged_atoms": 1, "average_repair_attempts": 1.0, "total_token_cost": 5}
    listed = cert_dir / "arithmetic" / "a.proof.json"
    listed.write_text(
        json.dumps({"file": "benchmarks/arithmetic/a.mm", "self_correction_summary": summary}),
        encoding="utf-8",
    )
    (cert_dir / "arithmetic" / "old.proof.json").write_text(
        json.dumps({"file": "benchmarks/arithmetic/old.mm", "self_correction_summary": summary}),
        encoding="utf-8",
    )
    # no manifest: every certificate is taken (legacy behaviour)
    assert set(suite.load_self_correction_summaries(cert_dir)) >= {"arithmetic/a.mm", "arithmetic/old.mm"}
    manifest = {
        "schema": suite.REPAIR_RUN_MANIFEST_SCHEMA,
        "files": [{"file": "arithmetic/a.mm", "certificate": str(listed)}],
    }
    (cert_dir / "run.json").write_text(json.dumps(manifest), encoding="utf-8")
    loaded = suite.load_self_correction_summaries(cert_dir)
    assert "arithmetic/a.mm" in loaded
    assert "arithmetic/old.mm" not in loaded


def test_lean_ai_proof_totals_roll_up_measured_categories_only(suite):
    blocks = [
        {
            "off": {"status": "MEASURED", "files": 1, "lean_verified_atoms": 4,
                    "manual_lemma_reason_remaining": 0, "lean_solver_time_s": 9.0},
            "on": {"status": "MEASURED", "files": 1, "lean_verified_atoms": 4, "ai_proof_used_atoms": 0,
                   "ai_proof_attempts": 0, "manual_lemma_reason_remaining": 0, "lean_solver_time_s": 7.0},
            "lean_verified_delta": 0,
        },
        {"off": {"status": "SKIP"}, "on": {"status": "SKIP", "files": 0}, "lean_verified_delta": None},
    ]
    totals = suite._lean_ai_proof_totals(blocks)
    assert totals["off"]["lean_verified_atoms"] == 4
    assert totals["on"]["lean_solver_time_s"] == 7.0
    assert totals["lean_verified_delta"] == 0
    assert totals["paired_categories"] == 1
    assert suite._lean_ai_proof_totals([blocks[1]])["off"] == {"status": "SKIP"}
    # off MEASURED but on SKIP: the off side is excluded from the paired totals too,
    # so a suite delta never compares totals over different category sets
    half = {
        "off": {"status": "MEASURED", "files": 1, "lean_verified_atoms": 9,
                "manual_lemma_reason_remaining": 0, "lean_solver_time_s": 1.0},
        "on": {"status": "SKIP", "files": 0},
        "lean_verified_delta": None,
    }
    totals = suite._lean_ai_proof_totals([blocks[0], half])
    assert totals["off"]["lean_verified_atoms"] == 4
    assert totals["lean_verified_delta"] == 0
    assert totals["unpaired_categories"] == 1


# ---------------------------------------------------------------------------
# Suite integration
# ---------------------------------------------------------------------------


def test_category_evaluation_without_binary_skips_solver_axes(suite):
    harness = suite.load_run_benchmarks()
    category = suite.evaluate_category(
        harness,
        "state_machine",
        harness.CATEGORIES["state_machine"],
        binary=None,
        lean_bridge=None,
        repair_summaries={},
        measure_artifacts=True,
    )
    axes = category["axes"]
    assert set(axes) == set(suite.AXES)
    assert axes["proof_success_rate"]["status"] == "SKIP"
    assert axes["proof_success_rate"]["success_rate"] is None
    assert axes["counterexample_quality"]["status"] == "SKIP"
    assert axes["repair_convergence"]["status"] == "SKIP"
    assert axes["runtime_artifact_utility"]["status"] == "SKIP"
    # static axes stay measured without a binary
    assert axes["user_burden"]["status"] == "MEASURED"
    assert axes["trust_surface"]["status"] == "MEASURED"
    assert axes["trust_surface"]["atoms"] > 0
    # escalation counts come from verifier output, so they are unavailable here
    assert axes["trust_surface"]["lean_escalation_candidates"] is None
    assert axes["trust_surface"]["lean_verified_atoms"] is None


def test_build_evaluation_reports_every_axis_and_is_deterministic(suite):
    harness = suite.load_run_benchmarks()
    categories = [
        suite.evaluate_category(
            harness,
            name,
            dir_path,
            binary=None,
            lean_bridge=None,
            repair_summaries={},
            measure_artifacts=False,
        )
        for name, dir_path in sorted(harness.CATEGORIES.items())
    ]
    evaluation = suite.build_evaluation(
        "2026-01-01 00:00 UTC",
        categories,
        {},
        budget_policy_fingerprint="sha256:test",
    )

    assert evaluation["schema"] == "mumei.evaluation_suite/v1"
    assert evaluation["axes"] == list(suite.AXES)
    assert evaluation["budget_policy_fingerprint"] == "sha256:test"
    assert set(evaluation["totals"]) == set(suite.AXES)
    assert [c["category"] for c in categories] == sorted(harness.CATEGORIES)

    totals = evaluation["totals"]
    assert totals["user_burden"]["status"] == "MEASURED"
    assert totals["trust_surface"]["status"] == "MEASURED"
    for axis in ("proof_success_rate", "repair_convergence", "runtime_artifact_utility"):
        assert totals[axis]["status"] == "SKIP"

    repeated = suite.build_evaluation(
        "2026-01-01 00:00 UTC",
        categories,
        {},
        budget_policy_fingerprint="sha256:test",
    )
    assert json.dumps(evaluation, sort_keys=True) == json.dumps(
        repeated, sort_keys=True
    )


def test_report_renders_skip_instead_of_substituted_values(suite):
    harness = suite.load_run_benchmarks()
    categories = [
        suite.evaluate_category(
            harness,
            "arithmetic",
            harness.CATEGORIES["arithmetic"],
            binary=None,
            lean_bridge=None,
            repair_summaries={},
            measure_artifacts=False,
        )
    ]
    evaluation = suite.build_evaluation("2026-01-01 00:00 UTC", categories, {})
    report = suite.format_report(evaluation)

    assert "| proof success rate | SKIP | SKIP |" in report
    assert "| repair convergence | SKIP | SKIP |" in report
    assert "| runtime artifact utility | SKIP | SKIP |" in report
    assert "| user burden | MEASURED |" in report
    assert "SKIP Lean escalation candidates" in report
    assert "`budget_policy_fingerprint`: `SKIP`" in report
    assert suite.format_report(evaluation) == report


def test_counterexample_axis_counts_only_counterexample_tasks_as_no_verdict(
    suite, monkeypatch
):
    harness = suite.load_run_benchmarks()
    dir_path = harness.CATEGORIES["arithmetic"]
    result = harness.run_category_benchmarks(None, "arithmetic", dir_path)
    passing = next(d for d in result["details"] if d["expected"] == "PASS")
    passing["verify_status"] = "TIMEOUT"
    result["no_verdict_files"] = 1
    result["no_verdict_statuses"] = ["TIMEOUT"]
    monkeypatch.setattr(
        harness, "run_category_benchmarks", lambda *a, **k: result
    )

    axes = suite.evaluate_category(
        harness,
        "arithmetic",
        dir_path,
        binary=None,
        lean_bridge=None,
        repair_summaries={},
        measure_artifacts=False,
    )["axes"]
    assert axes["proof_success_rate"]["no_verdict_files"] == 1
    # the outage hit an `expected: PASS` task, so it says nothing about the
    # counterexample tasks and must not appear in that axis
    assert axes["counterexample_quality"]["no_verdict_files"] == 0


def test_report_names_no_verdict_counts_for_both_verdict_axes(suite):
    payload = json.loads(
        (REPO_ROOT / "benchmarks" / "evaluation" / "evaluation_suite.json").read_text(
            encoding="utf-8"
        )
    )
    report = suite.format_report(payload)
    assert report.count("without a verdict") == 2


def test_markdown_output_accumulates_time_series(suite, tmp_path):
    output = tmp_path / "EVALUATION_SUITE.md"
    suite.append_report(output, "## Evaluation Suite Run — 1\n")
    suite.append_report(output, "## Evaluation Suite Run — 2\n")
    content = output.read_text(encoding="utf-8")
    assert content.startswith("# Evaluation Suite Results")
    assert content.count("## Evaluation Suite Run") == 2
    assert "\n---\n" in content


def test_committed_artifacts_match_the_schema():
    payload = json.loads(
        (REPO_ROOT / "benchmarks" / "evaluation" / "evaluation_suite.json").read_text(
            encoding="utf-8"
        )
    )
    suite_module = load_suite()
    assert payload["schema"] == suite_module.SCHEMA
    assert payload["axes"] == list(suite_module.AXES)
    assert set(payload["totals"]) == set(suite_module.AXES)
    for axis in payload["totals"].values():
        assert axis["status"] in {"MEASURED", "SKIP"}
    assert [c["category"] for c in payload["categories"]] == sorted(
        c["category"] for c in payload["categories"]
    )
    # Measured agent-backed axes must name the LLM that produced their
    # certificates; SKIP axes carry no provenance.
    agent_runs = payload["agent_runs"]
    assert set(agent_runs) == {"repair_convergence", "lean_ai_proof"}
    if payload["totals"]["repair_convergence"]["status"] == "MEASURED":
        assert agent_runs["repair_convergence"]["llm_model"]
    lean_ai = payload["totals"]["trust_surface"].get("lean_ai_proof") or {}
    if (lean_ai.get("on") or {}).get("status") == "MEASURED":
        assert agent_runs["lean_ai_proof"]["llm_model"]


def test_load_agent_run_manifest_keeps_only_provenance_keys(tmp_path):
    suite_module = load_suite()
    assert suite_module.load_agent_run_manifest(None) is None
    assert suite_module.load_agent_run_manifest(tmp_path) is None
    (tmp_path / "run.json").write_text(
        json.dumps({
            "schema": "mumei-agent.repair_convergence_run/v1",
            "llm_model": "qwen2.5-coder:3b",
            "llm_base_url": "http://localhost:11434/v1",
            "mumei_bin": "/home/someone/mumei",
            "max_retries": 3,
            "files": [{"file": "a.mm"}],
        }),
        encoding="utf-8",
    )
    assert suite_module.load_agent_run_manifest(tmp_path) == {
        "schema": "mumei-agent.repair_convergence_run/v1",
        "llm_model": "qwen2.5-coder:3b",
        "max_retries": 3,
    }
    (tmp_path / "run.json").write_text("not json", encoding="utf-8")
    assert suite_module.load_agent_run_manifest(tmp_path) is None


def test_committed_artifact_static_axes_are_not_stale():
    """The static axes are re-derivable, so the artifact must still match them.

    Guards against a benchmark or measurement change leaving obsolete numbers in
    the committed JSON while the shape checks keep passing.
    """
    suite_module = load_suite()
    harness = suite_module.load_run_benchmarks()
    payload = json.loads(
        (REPO_ROOT / "benchmarks" / "evaluation" / "evaluation_suite.json").read_text(
            encoding="utf-8"
        )
    )
    committed = {c["category"]: c for c in payload["categories"]}
    assert set(committed) == set(harness.CATEGORIES)

    for name, dir_path in sorted(harness.CATEGORIES.items()):
        sources = sorted(dir_path.glob("*.mm"))
        measured = suite_module.aggregate_user_burden(
            [suite_module.measure_user_burden(p) for p in sources]
        )
        recorded = committed[name]["axes"]["user_burden"]
        assert committed[name]["files"] == len(sources)
        for field in (
            "atoms",
            "spec_clauses",
            "spec_clause_kinds",
            "spec_clauses_per_atom",
            "spec_tokens",
            "impl_tokens",
            "spec_to_impl_token_ratio",
        ):
            assert recorded[field] == measured[field], f"{name}.{field} is stale"
    assert (REPO_ROOT / "docs" / "EVALUATION_SUITE.md").is_file()
