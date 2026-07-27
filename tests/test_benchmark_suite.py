"""Tests for benchmarks/run_benchmarks.py (P16 benchmark evaluation suite).

Covers the category registry, the ``expected: PASS`` / ``expected: FAIL``
classification of counterexample cases, and the zero-cost degradation of the
Lean escalation measurement when the mumei-lean bridge is unavailable.
"""
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SCRIPT_PATH = REPO_ROOT / "benchmarks" / "run_benchmarks.py"

MIN_CATEGORIES = 6
MIN_TOTAL_ATOMS = 60


def _load_module():
    spec = importlib.util.spec_from_file_location("run_benchmarks", SCRIPT_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_every_registered_category_exists():
    module = _load_module()
    assert len(module.CATEGORIES) >= MIN_CATEGORIES
    for name, dir_path in module.CATEGORIES.items():
        assert dir_path.is_dir(), f"category directory missing: {name}"
        assert sorted(dir_path.glob("*.mm")), f"category has no .mm files: {name}"


def test_every_benchmark_directory_is_registered():
    module = _load_module()
    registered = {p.resolve() for p in module.CATEGORIES.values()}
    for child in (REPO_ROOT / "benchmarks").iterdir():
        if child.is_dir() and sorted(child.glob("*.mm")):
            assert child.resolve() in registered, f"unregistered category: {child.name}"


def test_total_atom_count_meets_target():
    module = _load_module()
    total = 0
    for dir_path in module.CATEGORIES.values():
        for mm_file in sorted(dir_path.glob("*.mm")):
            total += module._count_atoms(mm_file)["total"]
    assert total >= MIN_TOTAL_ATOMS


def _category_result(name: str, **overrides) -> dict:
    result = {
        "category": name,
        "files": 4,
        "total_atoms": 10,
        "total_trusted": 0,
        "trusted_ratio": 0.0,
        "verified_count": 4,
        "matched_count": 4,
        "success_rate": 1.0,
        "counterexample_files": 1,
        "counterexamples_caught": 1,
        "counterexample_catch_rate": 1.0,
        "lean_measured_files": 0,
        "avg_lean_solver_time_s": None,
        "escalated_atoms": 0,
        "lean_verified_atoms": 0,
        "lean_discharge_rate": None,
        "tactic_search_adopted": 0,
        "avg_solver_time_s": 0.5,
        "details": [],
    }
    result.update(overrides)
    return result


def test_forge_feedback_scores_weak_categories():
    module = _load_module()
    feedback = module.build_forge_feedback(
        "2026-07-26 13:00 UTC",
        [
            _category_result("arithmetic", success_rate=0.5),
            _category_result("concurrency"),
        ],
        {"trusted_ratio": 0.12},
    )

    assert feedback["schema"] == module.FORGE_FEEDBACK_SCHEMA
    assert feedback["weak_categories"] == ["arithmetic"]
    by_name = {c["category"]: c for c in feedback["categories"]}
    assert by_name["arithmetic"]["weakness_score"] == 0.25
    assert by_name["arithmetic"]["priority_delta"] == -12
    assert "expected_outcome_mismatch" in by_name["arithmetic"]["signals"]
    # A perfect category contributes no bias.
    assert by_name["concurrency"]["weakness_score"] == 0.0
    assert by_name["concurrency"]["priority_delta"] == 0
    bias = {b["domain"]: b for b in feedback["domain_bias"]}
    assert bias["std/math"]["priority_delta"] == -12
    assert bias["std/math"]["driving_category"] == "arithmetic"
    assert bias["std/concurrency"]["priority_delta"] == 0


def test_forge_feedback_reports_solver_time_signals():
    module = _load_module()
    feedback = module.build_forge_feedback(
        "2026-07-26 13:00 UTC",
        [
            _category_result(
                "state_machine",
                trusted_ratio=0.5,
                counterexample_catch_rate=0.0,
                avg_solver_time_s=module.SLOW_SOLVER_TIME_S + 1,
                avg_lean_solver_time_s=module.SLOW_LEAN_SOLVER_TIME_S + 1,
                lean_measured_files=1,
            )
        ],
        {"trusted_ratio": 0.12},
    )

    [cat] = feedback["categories"]
    assert set(cat["signals"]) == {
        "counterexample_missed",
        "trusted_atoms_present",
        "z3_solver_time_pressure",
        "lean_escalation_cost",
    }
    # 0.3 * missed catch + 0.2 * trusted ratio
    assert cat["weakness_score"] == 0.4
    assert [b["domain"] for b in feedback["domain_bias"]] == [
        "std/contracts",
        "std/settlement",
    ]


def test_forge_feedback_generates_proposals_for_weak_categories():
    module = _load_module()
    feedback = module.build_forge_feedback(
        "2026-07-26 13:00 UTC",
        [
            _category_result(
                "arithmetic", success_rate=0.5, counterexample_catch_rate=0.0
            ),
            _category_result("concurrency"),
        ],
        {"trusted_ratio": 0.12},
    )

    # A healthy category never generates work.
    assert [p["driving_category"] for p in feedback["generated_proposals"]] == [
        "arithmetic"
    ]
    [proposal] = feedback["generated_proposals"]
    assert proposal["name"] == "std/math/benchmark_gaps.mm"
    assert proposal["source"] == "benchmark_forge_feedback"
    assert proposal["domain"] == "std/math"
    assert proposal["depends_on"] == ["std/prelude.mm"]
    assert proposal["signals"] == [
        "expected_outcome_mismatch",
        "counterexample_missed",
    ]
    assert [atom["name"] for atom in proposal["atoms"]] == [
        "math_bounded_result_guard",
        "math_counterexample_guard",
    ]
    for atom in proposal["atoms"]:
        assert atom["requires"] and atom["ensures"] and atom["return_type"] == "i64"


def test_generated_proposals_are_deterministic_and_signal_gated():
    module = _load_module()
    categories = [
        # Solver-time pressure alone is a cost report, not a coverage gap.
        _category_result(
            "svcomp_style", avg_solver_time_s=module.SLOW_SOLVER_TIME_S + 1
        ),
        # Below the weakness threshold: too weak a signal to generate work.
        _category_result("concurrency", trusted_ratio=0.01),
        _category_result("state_machine", trusted_ratio=0.9),
        _category_result("arithmetic", success_rate=0.0),
    ]
    args = ("2026-07-26 13:00 UTC", categories, {"trusted_ratio": 0.12})
    generated = module.build_forge_feedback(*args)["generated_proposals"]

    assert generated == module.build_forge_feedback(*args)["generated_proposals"]
    # Ordered by descending weakness, so the weakest domain is forged first.
    assert [p["driving_category"] for p in generated] == [
        "arithmetic",
        "state_machine",
    ]
    [trusted_atom] = generated[1]["atoms"]
    assert trusted_atom["name"] == "contracts_trusted_replacement"
    assert generated[1]["difficulty"] == "high"


def test_every_category_maps_to_std_domains():
    module = _load_module()
    assert set(module.CATEGORY_STD_DOMAINS) == set(module.CATEGORIES)


def test_counterexample_cases_declare_expected_fail():
    module = _load_module()
    counterexamples = 0
    for dir_path in module.CATEGORIES.values():
        for mm_file in sorted(dir_path.glob("*.mm")):
            expected = module._expected_outcome(mm_file)
            header = mm_file.read_text(encoding="utf-8")[:600]
            if mm_file.stem.endswith("_fail"):
                counterexamples += 1
                assert expected == "FAIL"
                assert "expected: FAIL" in header, (
                    f"{mm_file.name} must declare `expected: FAIL` in its header"
                )
            else:
                assert expected == "PASS"
                assert "expected: FAIL" not in header
    assert counterexamples >= 4


def test_every_new_category_has_success_and_counterexample_cases():
    module = _load_module()
    for name in ("arithmetic", "state_machine", "concurrency", "domain_compliance"):
        files = sorted(module.CATEGORIES[name].glob("*.mm"))
        outcomes = [module._expected_outcome(f) for f in files]
        assert outcomes.count("PASS") >= 2, f"{name} needs multiple success cases"
        assert outcomes.count("FAIL") >= 1, f"{name} needs counterexample cases"


def test_escalation_candidate_count_parsing():
    module = _load_module()
    assert module._escalation_candidate_count(
        "✅ Verification passed: 1 item(s) verified, 0 Lean escalation candidate(s)"
    ) == 0
    assert module._escalation_candidate_count(
        "✅ Verification passed: 1 item(s) verified, 2 Lean escalation candidate(s)"
    ) == 2
    assert module._escalation_candidate_count("") == 0


def test_lean_verified_and_tactic_search_parsing():
    module = _load_module()
    output = (
        "tactic search (build_failure) adopted `mumei_ff_mod` for atom "
        "ff_mul_assoc_bench in 2.424s\n"
        "tactic search (build_failure) exhausted 12 candidate(s) for atom other\n"
        "  lean_verified: ff_zero_is_zero_bench\n"
        "  lean_verified: ff_mul_assoc_bench\n"
        "  lean_verified: ff_mul_assoc_bench\n"
    )
    assert module._lean_verified_count(output) == 2
    assert module._tactic_search_adopted_count(output) == 1
    assert module._lean_verified_count("") == 0
    assert module._tactic_search_adopted_count("") == 0


def test_lean_discharge_rate_and_signal():
    module = _load_module()
    partial = _category_result(
        "arithmetic",
        escalated_atoms=4,
        lean_verified_atoms=3,
        lean_discharge_rate=0.75,
        tactic_search_adopted=2,
        lean_measured_files=1,
        avg_lean_solver_time_s=4.0,
    )
    assert "lean_escalation_undischarged" in module._category_signals(partial)
    full = _category_result(
        "arithmetic",
        escalated_atoms=4,
        lean_verified_atoms=4,
        lean_discharge_rate=1.0,
        tactic_search_adopted=2,
        lean_measured_files=1,
        avg_lean_solver_time_s=4.0,
    )
    assert "lean_escalation_undischarged" not in module._category_signals(full)
    feedback = module.build_forge_feedback(
        "2026-07-26 13:00 UTC", [full], {"trusted_ratio": 0.12}
    )
    [cat] = feedback["categories"]
    assert cat["lean_discharge_rate"] == 1.0
    assert cat["tactic_search_adopted"] == 2


def test_lean_bridge_resolution_degrades_to_none(tmp_path, monkeypatch):
    module = _load_module()
    monkeypatch.setenv("MUMEI_LEAN_PATH", str(tmp_path))
    assert module._resolve_lean_bridge() is None

    bridge = tmp_path / "scripts" / "bridge.py"
    bridge.parent.mkdir(parents=True)
    bridge.write_text("", encoding="utf-8")
    assert module._resolve_lean_bridge() == bridge


def test_category_aggregation_without_binary_is_zero_cost():
    module = _load_module()
    result = module.run_category_benchmarks(
        None, "arithmetic", module.CATEGORIES["arithmetic"], lean_bridge=None
    )
    assert result["category"] == "arithmetic"
    assert result["files"] == len(sorted(module.CATEGORIES["arithmetic"].glob("*.mm")))
    assert result["counterexample_files"] >= 1
    assert all(d["lean_status"] == "SKIP" for d in result["details"])
    assert all(d["lean_solver_time_s"] is None for d in result["details"])
    assert result["avg_lean_solver_time_s"] is None


def test_report_renders_counterexample_and_lean_columns():
    module = _load_module()
    category = {
        "category": "arithmetic",
        "files": 2,
        "total_atoms": 5,
        "total_trusted": 0,
        "trusted_ratio": 0.0,
        "verified_count": 1,
        "matched_count": 2,
        "success_rate": 1.0,
        "counterexample_files": 1,
        "counterexamples_caught": 1,
        "counterexample_catch_rate": 1.0,
        "lean_measured_files": 0,
        "avg_lean_solver_time_s": None,
        "escalated_atoms": 0,
        "lean_verified_atoms": 0,
        "lean_discharge_rate": None,
        "tactic_search_adopted": 0,
        "avg_solver_time_s": 0.5,
        "details": [
            {
                "file": "bounded_arithmetic.mm",
                "atoms": 4,
                "trusted": 0,
                "proven": 4,
                "verified": True,
                "expected": "PASS",
                "actual": "PASS",
                "matched": True,
                "solver_time_s": 0.5,
                "escalation_candidates": 0,
                "lean_solver_time_s": None,
                "lean_status": "SKIP",
                "lean_verified_atoms": 0,
                "tactic_search_adopted": 0,
            },
            {
                "file": "abs_min_int_fail.mm",
                "atoms": 1,
                "trusted": 0,
                "proven": 1,
                "verified": False,
                "expected": "FAIL",
                "actual": "FAIL",
                "matched": True,
                "solver_time_s": 0.5,
                "escalation_candidates": 0,
                "lean_solver_time_s": None,
                "lean_status": "SKIP",
                "lean_verified_atoms": 0,
                "tactic_search_adopted": 0,
            },
        ],
    }
    stdlib = {
        "modules": 1,
        "total_atoms": 4,
        "total_trusted": 0,
        "trusted_ratio": 0.0,
        "proven": 4,
    }
    report = module.format_report("2026-01-01 00:00 UTC", [category], stdlib)
    assert "Counterexample Catch" in report
    assert "Avg Lean Solver Time" in report
    assert "Lean Discharge" in report
    assert "Tactic Search" in report
    assert "100.00% (1/1)" in report
    assert "| FAIL | FAIL | yes |" in report.replace("  ", " ")
    assert "SKIP" in report
