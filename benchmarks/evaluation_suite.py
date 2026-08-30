"""P27 six-axis evaluation suite for the mumei verification pipeline.

`PAPER_DRAFT.md` §7 lists six evaluation axes. `benchmarks/run_benchmarks.py`
already measures three of them (proof success rate, counterexample quality, and
the trusted/proven ratio that makes up the trust surface); this suite reuses that
measurement verbatim and adds the remaining three so a single run reports all six
over the same controlled task set:

* **proof success rate** — share of benchmark files whose verification outcome
  matched `expected: PASS` / `expected: FAIL` (from ``run_benchmarks``);
* **repair convergence** — ``self_correction_summary`` (``convergence_rate`` /
  ``average_repair_attempts`` / ``total_token_cost``) aggregated from proof
  certificates produced by the mumei-agent self-correction loop;
* **counterexample quality** — share of `expected: FAIL` files the verifier
  rejects (from ``run_benchmarks``);
* **trust surface** — application `trusted atom` count, FFI boundary
  declarations, and the atoms escalating past Z3, counted with the same
  definitions as ``scripts/scale_trust_surface.py``;
* **user burden** — formal syntax the task demands: `requires` / `ensures` /
  `invariant` / `effect_pre` / `effect_post` clause counts and the specification
  to implementation token ratio, measured statically from the `.mm` source;
* **runtime artifact utility** — whether each task can actually emit its
  artifacts (LLVM IR, C header, verified JSON, proof certificate bundle).

Every axis degrades deterministically to ``SKIP`` when its input is missing —
the same policy the Lean solver-time measurement in ``run_benchmarks`` uses —
so a run without the `mumei` binary or without agent repair data still produces
a complete, reproducible document instead of a partial one.

The emitted JSON is a measurement artifact: it reuses the canonical
``budget_policy_fingerprint`` / ``self_correction_summary`` / ``lean_verified``
spellings and introduces no verdict or audit vocabulary.

Usage::

    python benchmarks/evaluation_suite.py \\
        --json benchmarks/evaluation/evaluation_suite.json \\
        --output docs/EVALUATION_SUITE.md
"""
from __future__ import annotations

import argparse
import datetime
import importlib.util
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
BENCHMARKS_DIR = REPO_ROOT / "benchmarks"
DEFAULT_JSON_OUTPUT = BENCHMARKS_DIR / "evaluation" / "evaluation_suite.json"
DEFAULT_MARKDOWN_OUTPUT = REPO_ROOT / "docs" / "EVALUATION_SUITE.md"

SCHEMA = "mumei.evaluation_suite/v1"

#: The six axes of `PAPER_DRAFT.md` §7, in the order the paper lists them.
AXES = (
    "proof_success_rate",
    "repair_convergence",
    "counterexample_quality",
    "trust_surface",
    "user_burden",
    "runtime_artifact_utility",
)

#: Measurement status vocabulary, identical to the Lean solver-time measurement
#: in ``run_benchmarks``: an axis is ``MEASURED`` when its input was present and
#: ``SKIP`` when it was absent. No new verdict vocabulary is introduced.
STATUS_MEASURED = "MEASURED"
STATUS_SKIP = "SKIP"

#: ``mumei build --emit`` targets plus the proof-certificate bundle, which is a
#: ``mumei verify`` output rather than a build target.
BUILD_EMIT_TARGETS = ("llvm-ir", "c-header", "verified-json")
PROOF_BUNDLE_TARGET = "proof-cert"
ARTIFACT_TARGETS = BUILD_EMIT_TARGETS + (PROOF_BUNDLE_TARGET,)

BUILD_TIMEOUT_S = 120

SPEC_CLAUSE_KINDS = ("requires", "ensures", "invariant", "effect_pre", "effect_post")
SPEC_CLAUSE_RE = re.compile(
    r"^\s*(" + "|".join(SPEC_CLAUSE_KINDS) + r"):",
)
BODY_START_RE = re.compile(r"^\s*body:")
ATOM_START_RE = re.compile(r"^\s*(?:trusted\s+|async\s+)?atom\s+\w+")
TOKEN_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*|\d+|[^\s\w]")


def load_run_benchmarks():
    """Load ``benchmarks/run_benchmarks.py`` as a module.

    The suite is a *consumer* of the benchmark harness: the three axes the
    harness already measures are taken from it unchanged so the two documents
    can never disagree.
    """
    script = BENCHMARKS_DIR / "run_benchmarks.py"
    spec = importlib.util.spec_from_file_location("run_benchmarks", script)
    if spec is None or spec.loader is None:  # pragma: no cover - packaging guard
        raise RuntimeError(f"cannot load benchmark harness: {script}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


# --------------------------------------------------------------------------
# Axis 5: user burden
# --------------------------------------------------------------------------


def _tokens(text: str) -> int:
    return len(TOKEN_RE.findall(text))


def _body_end(lines: list[str], start: int) -> int:
    """Index of the last line of the ``body:`` block opening at ``start``."""
    end = start
    depth = lines[start].count("{") - lines[start].count("}")
    while end < len(lines) and (
        depth > 0 or not lines[end].rstrip().endswith((";", "}"))
    ):
        end += 1
        if end >= len(lines):
            break
        depth += lines[end].count("{") - lines[end].count("}")
    return min(end, len(lines) - 1)


def measure_user_burden(path: Path) -> dict:
    """Count the formal syntax a task demands from its author.

    Two complementary readings of the same source, both purely static so the
    result never depends on solver behaviour:

    * ``spec_clauses`` — how many contract clauses the task has to state, and
      how many of them each atom carries on average;
    * ``spec_to_impl_token_ratio`` — specification tokens per implementation
      token, i.e. how much formal text buys one token of executable code. A
      body-less (`trusted`) atom contributes specification tokens only, so the
      ratio is reported as ``None`` when a file has no implementation tokens.

    Loop `invariant` clauses live inside a `body:` block, so body contents are
    scanned clause by clause: their tokens count as specification, not as
    implementation.
    """
    lines = path.read_text(encoding="utf-8").splitlines()
    clause_counts = {kind: 0 for kind in SPEC_CLAUSE_KINDS}
    spec_tokens = 0
    impl_tokens = 0
    atoms = 0

    index = 0
    while index < len(lines):
        line = lines[index]
        if ATOM_START_RE.match(line):
            atoms += 1
        clause = SPEC_CLAUSE_RE.match(line)
        if clause:
            end = index
            while end < len(lines) and not lines[end].rstrip().endswith(";"):
                end += 1
            end = min(end, len(lines) - 1)
            kind = clause.group(1)
            clause_counts[kind] += 1
            body_text = "\n".join(lines[index : end + 1]).split(":", 1)[1]
            spec_tokens += _tokens(body_text)
            index = end + 1
            continue
        if BODY_START_RE.match(line):
            end = _body_end(lines, index)
            body_lines = lines[index : end + 1]
            body_lines[0] = body_lines[0].split(":", 1)[1]
            for body_line in body_lines:
                nested = SPEC_CLAUSE_RE.match(body_line)
                if nested:
                    clause_counts[nested.group(1)] += 1
                    spec_tokens += _tokens(body_line.split(":", 1)[1])
                    continue
                impl_tokens += _tokens(body_line)
            index = end + 1
            continue
        index += 1

    total_clauses = sum(clause_counts.values())
    return {
        "file": path.name,
        "atoms": atoms,
        "spec_clauses": total_clauses,
        "spec_clause_kinds": clause_counts,
        "spec_clauses_per_atom": round(total_clauses / atoms, 4) if atoms else 0.0,
        "spec_tokens": spec_tokens,
        "impl_tokens": impl_tokens,
        "spec_to_impl_token_ratio": round(spec_tokens / impl_tokens, 4)
        if impl_tokens
        else None,
    }


def aggregate_user_burden(files: list[dict]) -> dict:
    """Aggregate per-file user-burden measurements over a category."""
    if not files:
        return {"status": STATUS_SKIP, "files": []}
    atoms = sum(f["atoms"] for f in files)
    clauses = sum(f["spec_clauses"] for f in files)
    spec_tokens = sum(f["spec_tokens"] for f in files)
    impl_tokens = sum(f["impl_tokens"] for f in files)
    kinds = {
        kind: sum(f["spec_clause_kinds"][kind] for f in files)
        for kind in SPEC_CLAUSE_KINDS
    }
    return {
        "status": STATUS_MEASURED,
        "atoms": atoms,
        "spec_clauses": clauses,
        "spec_clause_kinds": kinds,
        "spec_clauses_per_atom": round(clauses / atoms, 4) if atoms else 0.0,
        "spec_tokens": spec_tokens,
        "impl_tokens": impl_tokens,
        "spec_to_impl_token_ratio": round(spec_tokens / impl_tokens, 4)
        if impl_tokens
        else None,
        "files": files,
    }


# --------------------------------------------------------------------------
# Axis 2: repair convergence
# --------------------------------------------------------------------------


def load_self_correction_summaries(cert_dir: Path | None) -> dict[str, dict]:
    """Index ``self_correction_summary`` blocks by benchmark source.

    Each summary is indexed both by ``<category>/<file>.mm`` and by the bare
    file name. Two certificates for the same bare name in different categories
    drop the ambiguous bare key, so only the category-qualified lookup — the
    one ``evaluate_category`` uses — can resolve them.

    Proof certificates carry the summary only when the mumei-agent
    self-correction loop produced the atoms, so a plain local `mumei verify`
    run yields nothing here and the axis degrades to ``SKIP``.
    """
    if cert_dir is None or not cert_dir.is_dir():
        return {}
    summaries: dict[str, dict] = {}
    ambiguous: set[str] = set()
    for cert_path in sorted(cert_dir.rglob("*.json")):
        try:
            cert = json.loads(cert_path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            continue
        if not isinstance(cert, dict):
            continue
        summary = cert.get("self_correction_summary")
        source = cert.get("file")
        if not isinstance(summary, dict) or not isinstance(source, str):
            continue
        path = Path(source)
        if path.parent.name:
            summaries[f"{path.parent.name}/{path.name}"] = summary
        if path.name in summaries and summaries[path.name] != summary:
            ambiguous.add(path.name)
        summaries.setdefault(path.name, summary)
    for name in ambiguous:
        summaries.pop(name, None)
    return summaries


def _lookup_repair_summary(key: str, summaries: dict[str, dict]) -> dict | None:
    """Resolve a ``<category>/<file>`` key, falling back to the bare name."""
    if key in summaries:
        return summaries[key]
    return summaries.get(key.rsplit("/", 1)[-1])


def aggregate_repair_convergence(
    file_names: list[str],
    summaries: dict[str, dict],
) -> dict:
    """Aggregate ``SelfCorrectionSummary`` fields over a category.

    ``convergence_rate`` and ``average_repair_attempts`` are recomputed from the
    atom totals rather than averaged over files, matching
    ``SelfCorrectionSummary::from_atom_metadata`` in
    ``mumei-core/src/proof_cert/models.rs``.
    """
    matched = [
        (name, summary)
        for name, summary in (
            (name, _lookup_repair_summary(name, summaries)) for name in file_names
        )
        if summary is not None
    ]
    if not matched:
        return {"status": STATUS_SKIP, "files_with_repair_data": 0}
    total_atoms = sum(int(s.get("total_atoms", 0)) for _, s in matched)
    converged = sum(int(s.get("converged_atoms", 0)) for _, s in matched)
    attempts = sum(
        float(s.get("average_repair_attempts", 0.0)) * int(s.get("total_atoms", 0))
        for _, s in matched
    )
    tokens = sum(int(s.get("total_token_cost", 0)) for _, s in matched)
    return {
        "status": STATUS_MEASURED,
        "files_with_repair_data": len(matched),
        "total_atoms": total_atoms,
        "converged_atoms": converged,
        "convergence_rate": round(converged / total_atoms, 4) if total_atoms else 0.0,
        "average_repair_attempts": round(attempts / total_atoms, 4)
        if total_atoms
        else 0.0,
        "total_token_cost": tokens,
        "files": sorted(name for name, _ in matched),
    }


# --------------------------------------------------------------------------
# Axis 6: runtime artifact utility
# --------------------------------------------------------------------------


def _emitted_artifacts(work_dir: Path, prefix: str) -> list[str]:
    return sorted(
        p.name
        for p in work_dir.iterdir()
        if p.is_file() and p.name.startswith(prefix) and p.stat().st_size > 0
    )


def measure_runtime_artifacts(binary: str, source: Path, work_dir: Path) -> dict:
    """Emit every artifact target for one task and record which ones succeeded.

    Only `expected: PASS` tasks are emitted: a counterexample case is *supposed*
    to fail verification, so its inability to produce an artifact is the
    expected outcome rather than an artifact-utility signal.
    """
    targets: dict[str, dict] = {}
    for target in BUILD_EMIT_TARGETS:
        prefix = f"{source.stem}_{target}"
        try:
            proc = subprocess.run(
                [
                    binary,
                    "build",
                    str(source),
                    "-o",
                    str(work_dir / prefix),
                    "--emit",
                    target,
                ],
                capture_output=True,
                text=True,
                timeout=BUILD_TIMEOUT_S,
                cwd=str(work_dir),
            )
            emitted = _emitted_artifacts(work_dir, prefix)
            targets[target] = {
                "emitted": proc.returncode == 0 and bool(emitted),
                "artifacts": len(emitted),
            }
        except (subprocess.TimeoutExpired, FileNotFoundError):
            targets[target] = {"emitted": False, "artifacts": 0}

    cert_path = work_dir / f"{source.stem}.proof-cert.json"
    try:
        proc = subprocess.run(
            [binary, "verify", str(source), "--proof-cert", "--output", str(cert_path)],
            capture_output=True,
            text=True,
            timeout=BUILD_TIMEOUT_S,
            cwd=str(work_dir),
        )
        emitted = proc.returncode == 0 and cert_path.is_file() and cert_path.stat().st_size > 0
        targets[PROOF_BUNDLE_TARGET] = {
            "emitted": emitted,
            "artifacts": 1 if emitted else 0,
        }
    except (subprocess.TimeoutExpired, FileNotFoundError):
        targets[PROOF_BUNDLE_TARGET] = {"emitted": False, "artifacts": 0}

    return {
        "file": source.name,
        "targets": targets,
        "emitted_targets": sum(1 for t in targets.values() if t["emitted"]),
    }


def aggregate_runtime_artifacts(files: list[dict]) -> dict:
    """Aggregate emitter success over a category."""
    if not files:
        return {"status": STATUS_SKIP, "files": []}
    attempted = len(files) * len(ARTIFACT_TARGETS)
    emitted = sum(f["emitted_targets"] for f in files)
    per_target = {
        target: sum(1 for f in files if f["targets"][target]["emitted"])
        for target in ARTIFACT_TARGETS
    }
    return {
        "status": STATUS_MEASURED,
        "measured_files": len(files),
        "attempted_emissions": attempted,
        "successful_emissions": emitted,
        "emission_success_rate": round(emitted / attempted, 4) if attempted else 0.0,
        "per_target_success": per_target,
        "files": files,
    }


# --------------------------------------------------------------------------
# Axis 4: trust surface
# --------------------------------------------------------------------------


def aggregate_trust_surface(
    category_result: dict,
    sources: list[Path],
    *,
    binary_available: bool = True,
) -> dict:
    """Trust surface of a category: trusted atoms, FFI boundary, Lean escalation.

    Atom and trusted-atom counts come from the benchmark harness; the FFI
    boundary is counted with ``scripts/scale_trust_surface.py`` so the two
    artifacts report the same trust surface for the same source.

    Escalation and Lean-discharge counts are read out of verifier output, so
    without the `mumei` binary they are reported as unavailable rather than as
    zero; the static components stay measured either way.
    """
    sys.path.insert(0, str(REPO_ROOT / "scripts"))
    from scale_trust_surface import source_counts  # noqa: PLC0415

    extern_blocks = 0
    boundary_declarations = 0
    for source in sources:
        counts = source_counts(source)
        extern_blocks += counts["ffi_extern_blocks"]
        boundary_declarations += counts["ffi_boundary_declarations"]
    return {
        "status": STATUS_MEASURED,
        "atoms": category_result["total_atoms"],
        "application_trusted_atoms": category_result["total_trusted"],
        "trusted_ratio": category_result["trusted_ratio"],
        "ffi_extern_blocks": extern_blocks,
        "ffi_boundary_declarations": boundary_declarations,
        "lean_escalation_candidates": sum(
            d["escalation_candidates"] for d in category_result["details"]
        )
        if binary_available
        else None,
        "lean_verified_atoms": category_result["lean_verified_atoms"]
        if binary_available
        else None,
    }


# --------------------------------------------------------------------------
# Suite
# --------------------------------------------------------------------------


def evaluate_category(
    harness,
    category: str,
    dir_path: Path,
    *,
    binary: str | None,
    lean_bridge: Path | None,
    repair_summaries: dict[str, dict],
    measure_artifacts: bool,
) -> dict:
    """Measure all six axes for one controlled task category."""
    sources = sorted(dir_path.glob("*.mm"))
    result = harness.run_category_benchmarks(
        binary, category, dir_path, lean_bridge=lean_bridge
    )

    proof_success = {
        "status": STATUS_MEASURED if binary else STATUS_SKIP,
        "files": result["files"],
        "matched_files": result["matched_count"],
        "no_verdict_files": result["no_verdict_files"],
        "success_rate": result["success_rate"] if binary else None,
        "avg_solver_time_s": result["avg_solver_time_s"] if binary else None,
        "lean_discharge_rate": result["lean_discharge_rate"],
    }
    counterexample_quality = {
        "status": STATUS_MEASURED
        if binary and result["counterexample_files"]
        else STATUS_SKIP,
        "counterexample_files": result["counterexample_files"],
        "counterexamples_caught": result["counterexamples_caught"],
        "no_verdict_files": sum(
            1
            for detail in result["details"]
            if detail["expected"] == "FAIL"
            and detail["verify_status"] not in (STATUS_MEASURED, STATUS_SKIP)
        ),
        "counterexample_catch_rate": result["counterexample_catch_rate"]
        if binary
        else None,
    }

    burden = aggregate_user_burden([measure_user_burden(p) for p in sources])
    repair = aggregate_repair_convergence(
        [f"{category}/{p.name}" for p in sources], repair_summaries
    )
    trust = aggregate_trust_surface(
        result, sources, binary_available=bool(binary)
    )

    if binary and measure_artifacts:
        pass_sources = [p for p in sources if harness._expected_outcome(p) == "PASS"]
        work_dir = Path(tempfile.mkdtemp(prefix=f"mumei-eval-{category}-"))
        try:
            artifacts = aggregate_runtime_artifacts(
                [measure_runtime_artifacts(binary, p, work_dir) for p in pass_sources]
            )
        finally:
            shutil.rmtree(work_dir, ignore_errors=True)
    else:
        artifacts = {"status": STATUS_SKIP, "files": []}

    return {
        "category": category,
        "files": result["files"],
        "axes": {
            "proof_success_rate": proof_success,
            "repair_convergence": repair,
            "counterexample_quality": counterexample_quality,
            "trust_surface": trust,
            "user_burden": burden,
            "runtime_artifact_utility": artifacts,
        },
    }


def _optional_sum(values) -> int | None:
    """Sum counts, propagating unavailability instead of substituting zero."""
    collected = list(values)
    if any(value is None for value in collected):
        return None
    return sum(collected)


def _axis_totals(categories: list[dict]) -> dict:
    """Roll the per-category axes up into one suite-level figure per axis."""
    axes = [c["axes"] for c in categories]

    def measured(axis: str) -> list[dict]:
        return [a[axis] for a in axes if a[axis]["status"] == STATUS_MEASURED]

    def rate(numerator: int, denominator: int) -> float | None:
        return round(numerator / denominator, 4) if denominator else None

    proof = measured("proof_success_rate")
    counterexample = measured("counterexample_quality")
    burden = measured("user_burden")
    repair = measured("repair_convergence")
    artifacts = measured("runtime_artifact_utility")
    trust = measured("trust_surface")

    matched = sum(a["matched_files"] for a in proof)
    total_files = sum(a["files"] for a in proof)
    caught = sum(a["counterexamples_caught"] for a in counterexample)
    counterexample_files = sum(a["counterexample_files"] for a in counterexample)
    burden_atoms = sum(a["atoms"] for a in burden)
    burden_clauses = sum(a["spec_clauses"] for a in burden)
    spec_tokens = sum(a["spec_tokens"] for a in burden)
    impl_tokens = sum(a["impl_tokens"] for a in burden)
    repair_atoms = sum(a["total_atoms"] for a in repair)
    repair_converged = sum(a["converged_atoms"] for a in repair)
    repair_attempts = sum(
        a["average_repair_attempts"] * a["total_atoms"] for a in repair
    )
    emissions = sum(a["attempted_emissions"] for a in artifacts)
    emitted = sum(a["successful_emissions"] for a in artifacts)

    return {
        "proof_success_rate": {
            "status": STATUS_MEASURED if proof else STATUS_SKIP,
            "files": total_files,
            "matched_files": matched,
            "no_verdict_files": sum(a["no_verdict_files"] for a in proof),
            "success_rate": rate(matched, total_files),
        },
        "repair_convergence": {
            "status": STATUS_MEASURED if repair else STATUS_SKIP,
            "total_atoms": repair_atoms,
            "converged_atoms": repair_converged,
            "convergence_rate": rate(repair_converged, repair_atoms),
            "average_repair_attempts": round(repair_attempts / repair_atoms, 4)
            if repair_atoms
            else None,
            "total_token_cost": sum(a["total_token_cost"] for a in repair),
        },
        "counterexample_quality": {
            "status": STATUS_MEASURED if counterexample else STATUS_SKIP,
            "counterexample_files": counterexample_files,
            "counterexamples_caught": caught,
            "no_verdict_files": sum(a["no_verdict_files"] for a in counterexample),
            "counterexample_catch_rate": rate(caught, counterexample_files),
        },
        "trust_surface": {
            "status": STATUS_MEASURED if trust else STATUS_SKIP,
            "atoms": sum(a["atoms"] for a in trust),
            "application_trusted_atoms": sum(
                a["application_trusted_atoms"] for a in trust
            ),
            "ffi_boundary_declarations": sum(
                a["ffi_boundary_declarations"] for a in trust
            ),
            "lean_escalation_candidates": _optional_sum(
                a["lean_escalation_candidates"] for a in trust
            ),
        },
        "user_burden": {
            "status": STATUS_MEASURED if burden else STATUS_SKIP,
            "atoms": burden_atoms,
            "spec_clauses": burden_clauses,
            "spec_clauses_per_atom": rate(burden_clauses, burden_atoms),
            "spec_tokens": spec_tokens,
            "impl_tokens": impl_tokens,
            "spec_to_impl_token_ratio": rate(spec_tokens, impl_tokens),
        },
        "runtime_artifact_utility": {
            "status": STATUS_MEASURED if artifacts else STATUS_SKIP,
            "attempted_emissions": emissions,
            "successful_emissions": emitted,
            "emission_success_rate": rate(emitted, emissions),
        },
    }


def build_evaluation(
    timestamp: str,
    categories: list[dict],
    stdlib_metrics: dict,
    *,
    budget_policy_fingerprint: str | None = None,
) -> dict:
    """Assemble the ``mumei.evaluation_suite/v1`` document."""
    return {
        "schema": SCHEMA,
        "timestamp": timestamp,
        "budget_policy_fingerprint": budget_policy_fingerprint,
        "axes": list(AXES),
        "stdlib": stdlib_metrics,
        "totals": _axis_totals(categories),
        "categories": categories,
    }


def _fmt_rate(rate: float | None) -> str:
    return STATUS_SKIP if rate is None else f"{rate:.2%}"


def _fmt_number(value: float | None) -> str:
    return STATUS_SKIP if value is None else f"{value:.4f}"


def _fmt_count(value: int | None) -> str:
    return STATUS_SKIP if value is None else str(value)


def format_report(evaluation: dict) -> str:
    """Render the time-series markdown section for ``docs/EVALUATION_SUITE.md``."""
    totals = evaluation["totals"]
    fingerprint = evaluation["budget_policy_fingerprint"] or STATUS_SKIP

    def cell(axis: str, rendered: str) -> str:
        return STATUS_SKIP if totals[axis]["status"] == STATUS_SKIP else rendered

    lines = [
        f"## Evaluation Suite Run — {evaluation['timestamp']}",
        "",
        f"`budget_policy_fingerprint`: `{fingerprint}`. An axis reports `SKIP` when",
        "its input is absent (no `mumei` binary, or no agent repair data in the",
        "proof certificates), never a substituted value.",
        "",
        "### Axis Summary (PAPER_DRAFT.md §7)",
        "",
        "| Axis | Status | Result |",
        "|------|--------|--------|",
        f"| proof success rate | {totals['proof_success_rate']['status']} | "
        + cell(
            "proof_success_rate",
            f"{_fmt_rate(totals['proof_success_rate']['success_rate'])} "
            f"({totals['proof_success_rate']['matched_files']}"
            f"/{totals['proof_success_rate']['files']} files, "
            f"{totals['proof_success_rate']['no_verdict_files']} without a verdict)",
        )
        + " |",
        f"| repair convergence | {totals['repair_convergence']['status']} | "
        + cell(
            "repair_convergence",
            f"{_fmt_rate(totals['repair_convergence']['convergence_rate'])} "
            f"({totals['repair_convergence']['converged_atoms']}"
            f"/{totals['repair_convergence']['total_atoms']} atoms), "
            f"{_fmt_number(totals['repair_convergence']['average_repair_attempts'])} "
            "avg repair attempts",
        )
        + " |",
        f"| counterexample quality | {totals['counterexample_quality']['status']} | "
        + cell(
            "counterexample_quality",
            f"{_fmt_rate(totals['counterexample_quality']['counterexample_catch_rate'])} "
            f"({totals['counterexample_quality']['counterexamples_caught']}"
            f"/{totals['counterexample_quality']['counterexample_files']} caught, "
            f"{totals['counterexample_quality']['no_verdict_files']} without a verdict)",
        )
        + " |",
        f"| trust surface | {totals['trust_surface']['status']} | "
        + cell(
            "trust_surface",
            f"{totals['trust_surface']['application_trusted_atoms']} trusted "
            f"/ {totals['trust_surface']['atoms']} atoms, "
            f"{totals['trust_surface']['ffi_boundary_declarations']} FFI declarations, "
            f"{_fmt_count(totals['trust_surface']['lean_escalation_candidates'])} "
            "Lean escalation candidates",
        )
        + " |",
        f"| user burden | {totals['user_burden']['status']} | "
        + cell(
            "user_burden",
            f"{_fmt_number(totals['user_burden']['spec_clauses_per_atom'])} clauses/atom, "
            f"{_fmt_number(totals['user_burden']['spec_to_impl_token_ratio'])} "
            "spec/impl tokens",
        )
        + " |",
        f"| runtime artifact utility | {totals['runtime_artifact_utility']['status']} | "
        + cell(
            "runtime_artifact_utility",
            f"{_fmt_rate(totals['runtime_artifact_utility']['emission_success_rate'])} "
            f"({totals['runtime_artifact_utility']['successful_emissions']}"
            f"/{totals['runtime_artifact_utility']['attempted_emissions']} emissions)",
        )
        + " |",
        "",
        "### Per-Category Results",
        "",
        "| Category | Files | Success Rate | Counterexample Catch | Trusted / Atoms "
        "| Clauses/Atom | Spec/Impl Tokens | Artifact Emission | Repair Convergence |",
        "|----------|-------|--------------|----------------------|-----------------"
        "|--------------|------------------|-------------------|--------------------|",
    ]
    for category in evaluation["categories"]:
        axes = category["axes"]
        burden = axes["user_burden"]
        trust = axes["trust_surface"]
        repair = axes["repair_convergence"]
        artifacts = axes["runtime_artifact_utility"]
        repair_cell = (
            STATUS_SKIP
            if repair["status"] == STATUS_SKIP
            else _fmt_rate(repair["convergence_rate"])
        )
        artifact_cell = (
            STATUS_SKIP
            if artifacts["status"] == STATUS_SKIP
            else f"{_fmt_rate(artifacts['emission_success_rate'])} "
            f"({artifacts['successful_emissions']}/{artifacts['attempted_emissions']})"
        )
        lines.append(
            f"| {category['category']} | {category['files']} "
            f"| {_fmt_rate(axes['proof_success_rate']['success_rate'])} "
            f"| {_fmt_rate(axes['counterexample_quality']['counterexample_catch_rate'])} "
            f"| {trust['application_trusted_atoms']} / {trust['atoms']} "
            f"| {_fmt_number(burden['spec_clauses_per_atom'])} "
            f"| {_fmt_number(burden['spec_to_impl_token_ratio'])} "
            f"| {artifact_cell} | {repair_cell} |"
        )
    lines.extend([
        "",
        "### User Burden by Clause Kind",
        "",
        "| Category | requires | ensures | invariant | effect_pre | effect_post "
        "| Spec Tokens | Impl Tokens |",
        "|----------|---------:|--------:|----------:|-----------:|------------:"
        "|------------:|------------:|",
    ])
    for category in evaluation["categories"]:
        burden = category["axes"]["user_burden"]
        kinds = burden["spec_clause_kinds"]
        lines.append(
            f"| {category['category']} | {kinds['requires']} | {kinds['ensures']} "
            f"| {kinds['invariant']} | {kinds['effect_pre']} | {kinds['effect_post']} "
            f"| {burden['spec_tokens']} | {burden['impl_tokens']} |"
        )
    lines.extend([
        "",
        "### Runtime Artifact Utility by Target",
        "",
        "| Category | "
        + " | ".join(f"`{target}`" for target in ARTIFACT_TARGETS)
        + " | Files |",
        "|----------|" + "------|" * (len(ARTIFACT_TARGETS) + 1),
    ])
    for category in evaluation["categories"]:
        artifacts = category["axes"]["runtime_artifact_utility"]
        if artifacts["status"] == STATUS_SKIP:
            cells = " | ".join(STATUS_SKIP for _ in ARTIFACT_TARGETS)
            lines.append(f"| {category['category']} | {cells} | 0 |")
            continue
        cells = " | ".join(
            str(artifacts["per_target_success"][target]) for target in ARTIFACT_TARGETS
        )
        lines.append(
            f"| {category['category']} | {cells} | {artifacts['measured_files']} |"
        )
    gaps = [
        (category["category"], entry["file"], target)
        for category in evaluation["categories"]
        for entry in category["axes"]["runtime_artifact_utility"].get("files", [])
        for target in ARTIFACT_TARGETS
        if not entry["targets"][target]["emitted"]
    ]
    if gaps:
        lines.extend([
            "",
            "### Artifact Emission Gaps",
            "",
            "| Category | File | Target |",
            "|----------|------|--------|",
        ])
        lines.extend(
            f"| {category} | `{name}` | `{target}` |"
            for category, name, target in gaps
        )
    lines.append("")
    return "\n".join(lines)


MARKDOWN_HEADER = (
    "# Evaluation Suite Results\n\n"
    "Time-series results of the six-axis evaluation suite of `PAPER_DRAFT.md` §7,\n"
    "measured by `benchmarks/evaluation_suite.py` over the controlled benchmark\n"
    "task set in `benchmarks/`.\n\n"
)


def append_report(output_path: Path, report: str) -> None:
    """Append a run to the time-series document, as ``run_benchmarks`` does."""
    if output_path.exists():
        existing = output_path.read_text(encoding="utf-8")
        if existing.startswith("# Evaluation Suite Results"):
            content = existing.rstrip("\n") + "\n\n---\n\n" + report
        else:
            content = MARKDOWN_HEADER + report
    else:
        content = MARKDOWN_HEADER + report
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(content, encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--json",
        type=Path,
        default=DEFAULT_JSON_OUTPUT,
        help="Path for the mumei.evaluation_suite/v1 JSON artifact",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_MARKDOWN_OUTPUT,
        help="Time-series markdown summary to append to",
    )
    parser.add_argument(
        "--repair-cert-dir",
        type=Path,
        default=None,
        help=(
            "Directory of proof certificates carrying self_correction_summary "
            "(mumei-agent repair runs); without it repair convergence is SKIP"
        ),
    )
    parser.add_argument("--budget-policy-fingerprint", default=None)
    parser.add_argument(
        "--no-lean",
        action="store_true",
        help="Never invoke the mumei-lean bridge (passed through to the harness)",
    )
    parser.add_argument(
        "--no-artifacts",
        action="store_true",
        help="Skip the runtime artifact utility axis; it reports SKIP",
    )
    args = parser.parse_args(argv)

    harness = load_run_benchmarks()
    binary = harness._find_mumei_binary()
    if not binary:
        print(
            "mumei binary not found; solver-dependent axes will be SKIP",
            file=sys.stderr,
        )
    lean_bridge = None if args.no_lean else harness._resolve_lean_bridge()
    repair_summaries = load_self_correction_summaries(args.repair_cert_dir)
    if not repair_summaries:
        print(
            "no self_correction_summary found; repair convergence will be SKIP",
            file=sys.stderr,
        )

    timestamp = datetime.datetime.now(datetime.timezone.utc).strftime(
        "%Y-%m-%d %H:%M UTC"
    )
    categories = [
        evaluate_category(
            harness,
            name,
            dir_path,
            binary=binary,
            lean_bridge=lean_bridge,
            repair_summaries=repair_summaries,
            measure_artifacts=not args.no_artifacts,
        )
        for name, dir_path in sorted(harness.CATEGORIES.items())
        if dir_path.is_dir()
    ]
    evaluation = build_evaluation(
        timestamp,
        categories,
        harness.collect_stdlib_metrics(),
        budget_policy_fingerprint=args.budget_policy_fingerprint,
    )

    args.json.parent.mkdir(parents=True, exist_ok=True)
    args.json.write_text(
        json.dumps(evaluation, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"wrote {args.json}")

    append_report(args.output, format_report(evaluation))
    print(f"wrote {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
