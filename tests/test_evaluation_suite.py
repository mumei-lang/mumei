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


def test_status_vocabulary_is_reused_not_extended(suite):
    assert suite.STATUS_MEASURED == "MEASURED"
    assert suite.STATUS_SKIP == "SKIP"
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
