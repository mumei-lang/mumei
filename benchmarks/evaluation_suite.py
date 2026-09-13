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
#: ``SKIP`` when it was absent. ``INCOMPLETE`` is used only for the B-7 AI-on
#: side when its certificates cover part of the off-side candidate set (the
#: two sides are then not comparable and no delta is reported). None of these
#: touch the atom-level verdict vocabulary.
STATUS_MEASURED = "MEASURED"
STATUS_SKIP = "SKIP"
STATUS_INCOMPLETE = "INCOMPLETE"

#: ``mumei build --emit`` targets plus the proof-certificate bundle, which is a
#: ``mumei verify`` output rather than a build target.
BUILD_EMIT_TARGETS = ("llvm-ir", "c-header", "verified-json")
PROOF_BUNDLE_TARGET = "proof-cert"
ARTIFACT_TARGETS = BUILD_EMIT_TARGETS + (PROOF_BUNDLE_TARGET,)

BUILD_TIMEOUT_S = 120
LEAN_VERIFY_TIMEOUT_S = 300

#: ``mumei verify`` verdict exit codes (R-1), mirrored from ``run_benchmarks``.
EXIT_VERIFIED = 0
EXIT_REJECTED = 1
EXIT_INCONCLUSIVE = 3

#: ``z3_check_result`` value written by the mumei-lean bridge when Lean closed
#: an obligation (the only value that upgrades an ``unknown`` atom).
LEAN_VERIFIED = "lean_verified"

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

    When the directory carries the repair-run manifest (``run.json`` with
    ``schema`` ``mumei-agent.repair_convergence_run/v1``) only the certificates
    it lists are indexed, so leftovers from an earlier run in a reused directory
    cannot be attributed to the run recorded under ``agent_runs``.
    """
    if cert_dir is None or not cert_dir.is_dir():
        return {}
    allowed = _manifest_certificate_files(cert_dir, REPAIR_RUN_MANIFEST_SCHEMA)
    if allowed is not None and not allowed:
        return {}
    summaries: dict[str, dict] = {}
    ambiguous: set[str] = set()
    for cert_path in sorted(cert_dir.rglob("*.json")):
        if allowed is not None and cert_path.resolve() not in allowed:
            continue
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


AGENT_RUN_MANIFEST = "run.json"
REPAIR_RUN_MANIFEST_SCHEMA = "mumei-agent.repair_convergence_run/v1"
AGENT_RUN_MANIFEST_KEYS = (
    "schema",
    "llm_model",
    "max_retries",
    "lean_ai_proof_max_attempts",
)


def load_agent_run_manifest(cert_dir: Path | None) -> dict | None:
    """Read the provenance manifest mumei-agent writes next to its certificates.

    Only the model / retry-budget keys are copied so the committed artifact
    records *which* LLM produced the repair or AI-proof certificates without
    leaking local paths or endpoints. Absent manifest -> ``None``.
    """
    if cert_dir is None:
        return None
    manifest_path = cert_dir / AGENT_RUN_MANIFEST
    if not manifest_path.is_file():
        return None
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError):
        return None
    if not isinstance(manifest, dict):
        return None
    return {key: manifest[key] for key in AGENT_RUN_MANIFEST_KEYS if key in manifest}


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


def _expected_emission(expected: str, target: str) -> bool:
    """Whether an emitter is *supposed* to hand out ``target`` for a task.

    A verified (`expected: PASS`) task must yield every artifact.  A
    counterexample (`expected: FAIL`) task must yield **no** executable artifact
    -- the build gate is the runtime utility here -- while the proof bundle is
    still expected, because the certificate is what carries the refutation
    (``verification_status`` / ``contradiction_type``) to downstream tools.
    """
    if expected == "PASS":
        return True
    return target == PROOF_BUNDLE_TARGET


def _leaked(record: dict) -> bool:
    """A build target leaked when *any* non-empty artifact exists, whatever the exit code."""
    return record.get("artifacts", 0) > 0


def _as_expected(expected: str, target: str, record: dict) -> bool:
    if _expected_emission(expected, target):
        return bool(record["emitted"])
    return not _leaked(record)


def measure_runtime_artifacts(
    binary: str,
    source: Path,
    work_dir: Path,
    *,
    expected: str = "PASS",
    lean_bridge: Path | None = None,
) -> dict:
    """Emit every artifact target for one task and record which ones succeeded.

    ``expected`` is the task's declared outcome.  Each target records whether
    an artifact was ``emitted`` and whether that matches what the task demands
    (``as_expected``, see :func:`_expected_emission`), so counterexample tasks
    contribute a measurable "refused to build, still certified" signal instead
    of being dropped from the axis.

    The proof-certificate target follows the harness verdict rule: a plain
    ``mumei verify --proof-cert`` that exits ``EXIT_INCONCLUSIVE`` (Z3 left
    Lean escalation candidates open) is re-run with ``--escalate-lean`` when
    ``lean_bridge`` is available, and that run's exit code decides whether the
    certificate counts as emitted.  Without a bridge the inconclusive run is
    not a verdict and no certificate is credited.
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
        if proc.returncode == EXIT_INCONCLUSIVE and lean_bridge is not None:
            cert_path.unlink(missing_ok=True)
            proc = subprocess.run(
                [
                    binary,
                    "verify",
                    str(source),
                    "--proof-cert",
                    "--escalate-lean",
                    "--output",
                    str(cert_path),
                ],
                capture_output=True,
                text=True,
                timeout=LEAN_VERIFY_TIMEOUT_S,
                cwd=str(work_dir),
            )
        verdict_ok = proc.returncode == (
            EXIT_VERIFIED if expected == "PASS" else EXIT_REJECTED
        )
        emitted = verdict_ok and cert_path.is_file() and cert_path.stat().st_size > 0
        targets[PROOF_BUNDLE_TARGET] = {
            "emitted": emitted,
            "artifacts": 1 if emitted else 0,
        }
    except (subprocess.TimeoutExpired, FileNotFoundError):
        targets[PROOF_BUNDLE_TARGET] = {"emitted": False, "artifacts": 0}

    for target, record in targets.items():
        record["as_expected"] = _as_expected(expected, target, record)

    return {
        "file": source.name,
        "expected": expected,
        "targets": targets,
        "emitted_targets": sum(1 for t in targets.values() if t["emitted"]),
        "as_expected_targets": sum(1 for t in targets.values() if t["as_expected"]),
    }


def aggregate_runtime_artifacts(files: list[dict]) -> dict:
    """Aggregate emitter success over a category."""
    if not files:
        return {"status": STATUS_SKIP, "files": []}
    pass_files = [f for f in files if f.get("expected", "PASS") == "PASS"]
    fail_files = [f for f in files if f.get("expected") == "FAIL"]
    attempted = len(pass_files) * len(ARTIFACT_TARGETS)
    emitted = sum(f["emitted_targets"] for f in pass_files)
    per_target = {
        target: sum(1 for f in pass_files if f["targets"][target]["emitted"])
        for target in ARTIFACT_TARGETS
    }
    ce_attempted = len(fail_files) * len(ARTIFACT_TARGETS)
    ce_as_expected = sum(
        1
        for f in fail_files
        for target in ARTIFACT_TARGETS
        if _as_expected("FAIL", target, f["targets"][target])
    )
    ce_leaked = sum(
        1
        for f in fail_files
        for target in BUILD_EMIT_TARGETS
        if _leaked(f["targets"][target])
    )
    ce_certified = sum(
        1 for f in fail_files if f["targets"][PROOF_BUNDLE_TARGET]["emitted"]
    )
    return {
        "status": STATUS_MEASURED,
        "measured_files": len(files),
        "attempted_emissions": attempted,
        "successful_emissions": emitted,
        "emission_success_rate": round(emitted / attempted, 4) if attempted else 0.0,
        "per_target_success": per_target,
        "counterexample": {
            "files": len(fail_files),
            "attempted_emissions": ce_attempted,
            "as_expected_emissions": ce_as_expected,
            "as_expected_rate": round(ce_as_expected / ce_attempted, 4)
            if ce_attempted
            else None,
            "leaked_build_artifacts": ce_leaked,
            "refutation_certificates": ce_certified,
        },
        "files": files,
    }


# --------------------------------------------------------------------------
# Axis 4: trust surface
# --------------------------------------------------------------------------


AI_PROOF_RUN_MANIFEST_SCHEMA = "mumei-agent.lean_ai_proof_run/v1"


def _manifest_certificate_files(cert_dir: Path, schema: str) -> set[Path] | None:
    """Certificate paths a mumei-agent run manifest of ``schema`` vouches for.

    ``None`` when there is no manifest of that schema; an empty set when the
    manifest is an AI-*off* run or lists no certificates. Relative certificate
    paths resolve against the manifest's directory.
    """
    manifest_path = cert_dir / AGENT_RUN_MANIFEST
    if not manifest_path.is_file():
        return None
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError):
        return None
    if not isinstance(manifest, dict) or manifest.get("schema") != schema:
        return None
    if manifest.get("ai_proof", "on") != "on":
        return set()
    files = manifest.get("files")
    if not isinstance(files, list):
        return set()
    allowed: set[Path] = set()
    for entry in files:
        if not isinstance(entry, dict):
            continue
        cert = entry.get("certificate")
        if isinstance(cert, str) and cert:
            allowed.add((cert_dir / cert).resolve())
    return allowed


def _ai_proof_manifest_files(cert_dir: Path) -> set[Path] | None:
    return _manifest_certificate_files(cert_dir, AI_PROOF_RUN_MANIFEST_SCHEMA)


def load_ai_proof_certificates(cert_dir: Path | None) -> dict[str, dict]:
    """Index proof certificates produced with the AI Lean proof path *on*.

    These are the certificates mumei-agent writes after running the mumei-lean
    bridge with ``--enable-lean-ai-proof`` (``scripts/measure_lean_ai_proof.py``
    in mumei-agent), keyed by ``<category>/<file>.mm`` exactly like the repair
    certificates. The suite's own harness run is the AI-*off* measurement, so
    the two sides share the escalation-candidate denominator.

    When the directory carries the measurement manifest (``run.json`` with
    ``schema`` ``mumei-agent.lean_ai_proof_run/v1``) only the certificates that
    manifest lists are loaded, and none at all if it records an AI-*off* run:
    stray or stale certificate-shaped JSON under the directory must not inherit
    the manifest's provenance. Without a manifest every certificate is taken.
    """
    if cert_dir is None or not cert_dir.is_dir():
        return {}
    allowed = _ai_proof_manifest_files(cert_dir)
    if allowed is not None and not allowed:
        return {}
    certs: dict[str, dict] = {}
    for cert_path in sorted(cert_dir.rglob("*.json")):
        if allowed is not None and cert_path.resolve() not in allowed:
            continue
        try:
            cert = json.loads(cert_path.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError):
            continue
        if not isinstance(cert, dict) or not isinstance(cert.get("atoms"), list):
            continue
        source = cert.get("file")
        if not isinstance(source, str):
            continue
        path = Path(source)
        if path.parent.name:
            certs[f"{path.parent.name}/{path.name}"] = cert
    return certs


def _lean_metadata(atom: dict) -> dict:
    for key in ("lean_metadata", "lean_result_metadata"):
        value = atom.get(key)
        if isinstance(value, dict):
            return value
    return {}


def summarize_ai_proof_certificate(cert: dict, harness) -> dict:
    """Per-file Lean figures read out of an AI-on proof certificate.

    ``ai_proof_used`` / ``ai_proof_attempts`` are the provenance keys the
    mumei-lean bridge writes into ``lean_metadata``; nothing is inferred from
    logs on this side.
    """
    atoms = [a for a in cert.get("atoms", []) if isinstance(a, dict)]
    solver_times = [
        float(_lean_metadata(a)["lean_solver_time_s"])
        for a in atoms
        if isinstance(_lean_metadata(a).get("lean_solver_time_s"), (int, float))
    ]
    return {
        "lean_verified_atoms": sum(
            1 for a in atoms if a.get("z3_check_result") == LEAN_VERIFIED
        ),
        "ai_proof_used_atoms": sum(
            1
            for a in atoms
            if _lean_metadata(a).get("ai_proof_used") is True
            and a.get("z3_check_result") == LEAN_VERIFIED
        ),
        "ai_proof_attempts": sum(
            int(_lean_metadata(a).get("ai_proof_attempts") or 0) for a in atoms
        ),
        "manual_lemma_reason_remaining": harness.manual_lemma_reason_remaining(cert),
        "lean_solver_time_s": round(max(solver_times), 3) if solver_times else None,
    }


def aggregate_lean_ai_proof(
    category: str,
    category_result: dict,
    ai_proof_certs: dict[str, dict],
    harness,
    *,
    binary_available: bool = True,
) -> dict:
    """B-7: AI Lean proof path on/off over the category's escalation candidates.

    ``off`` is the harness's own ``--escalate-lean`` measurement (tactic ladder
    + known witnesses only). ``on`` is read from the certificates in
    ``--ai-proof-cert-dir`` for the same files; it is ``SKIP`` when none were
    supplied and ``INCOMPLETE`` (no delta) when certificates cover only part of
    the candidate set, because the two sides are only comparable over identical
    files. ``lean_verified_delta`` is the number of atoms the AI path closed on
    top of the off path.
    """
    candidates = [
        d for d in category_result["details"] if d["escalation_candidates"]
    ]
    if not binary_available:
        off: dict = {"status": STATUS_SKIP}
    else:
        off_times = [
            d["lean_solver_time_s"] for d in candidates
            if d["lean_solver_time_s"] is not None
        ]
        unmeasured = sorted(
            (d["file"], d.get("lean_status", STATUS_SKIP))
            for d in candidates
            if d.get("lean_status") != STATUS_MEASURED
        )
        if not off_times:
            off_status = STATUS_SKIP
        elif unmeasured:
            off_status = STATUS_INCOMPLETE
        else:
            off_status = STATUS_MEASURED
        off = {
            "status": off_status,
            "unmeasured_files": [list(pair) for pair in unmeasured],
            "files": len(candidates),
            "lean_verified_atoms": sum(d["lean_verified_atoms"] for d in candidates),
            "manual_lemma_reason_remaining": _optional_sum(
                d.get("manual_lemma_reason_remaining") for d in candidates
            ),
            "lean_solver_time_s": round(sum(off_times), 3) if off_times else None,
        }
    matched = [
        (d["file"], ai_proof_certs[f"{category}/{d['file']}"])
        for d in candidates
        if f"{category}/{d['file']}" in ai_proof_certs
    ]
    missing = sorted(
        d["file"] for d in candidates if f"{category}/{d['file']}" not in ai_proof_certs
    )
    off_hashes = {d["file"]: d.get("atom_content_hashes") for d in candidates}
    stale = sorted(
        name
        for name, cert in matched
        if off_hashes.get(name) is not None
        and harness.atom_content_hashes(cert) != off_hashes[name]
    )
    if not matched:
        on: dict = {"status": STATUS_SKIP, "files": 0}
    else:
        per_file = [summarize_ai_proof_certificate(c, harness) for _, c in matched]
        on_times = [
            s["lean_solver_time_s"] for s in per_file
            if s["lean_solver_time_s"] is not None
        ]
        on = {
            "status": STATUS_INCOMPLETE if missing or stale else STATUS_MEASURED,
            "files": len(matched),
            "missing_files": missing,
            "stale_files": stale,
            "lean_verified_atoms": sum(s["lean_verified_atoms"] for s in per_file),
            "ai_proof_used_atoms": sum(s["ai_proof_used_atoms"] for s in per_file),
            "ai_proof_attempts": sum(s["ai_proof_attempts"] for s in per_file),
            "manual_lemma_reason_remaining": _optional_sum(
                s["manual_lemma_reason_remaining"] for s in per_file
            ),
            "lean_solver_time_s": round(sum(on_times), 3) if on_times else None,
            "file_names": sorted(name for name, _ in matched),
        }
    delta = None
    if off["status"] == STATUS_MEASURED and on["status"] == STATUS_MEASURED:
        delta = on["lean_verified_atoms"] - off["lean_verified_atoms"]
    return {"off": off, "on": on, "lean_verified_delta": delta}


def aggregate_trust_surface(
    category_result: dict,
    sources: list[Path],
    *,
    binary_available: bool = True,
    lean_ai_proof: dict | None = None,
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
        "manual_lemma_reason_remaining": category_result.get(
            "manual_lemma_reason_remaining"
        )
        if binary_available
        else None,
        "lean_ai_proof": lean_ai_proof,
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
    ai_proof_certs: dict[str, dict] | None = None,
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
        result,
        sources,
        binary_available=bool(binary),
        lean_ai_proof=aggregate_lean_ai_proof(
            category,
            result,
            ai_proof_certs or {},
            harness,
            binary_available=bool(binary),
        ),
    )

    if binary and measure_artifacts:
        work_dir = Path(tempfile.mkdtemp(prefix=f"mumei-eval-{category}-"))
        try:
            artifacts = aggregate_runtime_artifacts(
                [
                    measure_runtime_artifacts(
                        binary,
                        p,
                        work_dir,
                        expected=harness._expected_outcome(p),
                        lean_bridge=lean_bridge,
                    )
                    for p in sources
                ]
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


def _lean_ai_proof_totals(blocks: list[dict]) -> dict:
    """Roll the per-category B-7 on/off blocks up to suite level.

    Both sides are summed over the *same* categories -- those where off and on
    are each ``MEASURED`` -- so the suite delta never compares an off total
    that includes categories the AI run did not cover.
    """
    paired = [
        b for b in blocks
        if b["off"]["status"] == STATUS_MEASURED and b["on"]["status"] == STATUS_MEASURED
    ]

    def side(name: str, keys: tuple[str, ...]) -> dict:
        measured = [b[name] for b in paired]
        if not measured:
            return {"status": STATUS_SKIP}
        total: dict = {"status": STATUS_MEASURED, "files": sum(m["files"] for m in measured)}
        for key in keys:
            total[key] = _optional_sum(m.get(key) for m in measured)
        times = [m["lean_solver_time_s"] for m in measured if m["lean_solver_time_s"] is not None]
        total["lean_solver_time_s"] = round(sum(times), 3) if times else None
        return total

    off = side("off", ("lean_verified_atoms", "manual_lemma_reason_remaining"))
    on = side(
        "on",
        (
            "lean_verified_atoms",
            "ai_proof_used_atoms",
            "ai_proof_attempts",
            "manual_lemma_reason_remaining",
        ),
    )
    delta = None
    if off["status"] == STATUS_MEASURED and on["status"] == STATUS_MEASURED:
        delta = on["lean_verified_atoms"] - off["lean_verified_atoms"]
    return {
        "off": off,
        "on": on,
        "lean_verified_delta": delta,
        "paired_categories": len(paired),
        "unpaired_categories": len(blocks) - len(paired),
    }


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
    ce_emissions = sum(a["counterexample"]["attempted_emissions"] for a in artifacts)
    ce_as_expected = sum(a["counterexample"]["as_expected_emissions"] for a in artifacts)

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
            "lean_verified_atoms": _optional_sum(
                a["lean_verified_atoms"] for a in trust
            ),
            "manual_lemma_reason_remaining": _optional_sum(
                a.get("manual_lemma_reason_remaining")
                for a in trust
                if a["lean_escalation_candidates"]
            ),
            "lean_ai_proof": _lean_ai_proof_totals(
                [a["lean_ai_proof"] for a in trust if a.get("lean_ai_proof")]
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
            "counterexample": {
                "files": sum(a["counterexample"]["files"] for a in artifacts),
                "attempted_emissions": ce_emissions,
                "as_expected_emissions": ce_as_expected,
                "as_expected_rate": rate(ce_as_expected, ce_emissions),
                "leaked_build_artifacts": sum(
                    a["counterexample"]["leaked_build_artifacts"] for a in artifacts
                ),
                "refutation_certificates": sum(
                    a["counterexample"]["refutation_certificates"] for a in artifacts
                ),
            },
        },
    }


def build_evaluation(
    timestamp: str,
    categories: list[dict],
    stdlib_metrics: dict,
    *,
    budget_policy_fingerprint: str | None = None,
    agent_runs: dict[str, dict | None] | None = None,
) -> dict:
    """Assemble the ``mumei.evaluation_suite/v1`` document."""
    return {
        "schema": SCHEMA,
        "timestamp": timestamp,
        "budget_policy_fingerprint": budget_policy_fingerprint,
        "agent_runs": agent_runs
        or {"repair_convergence": None, "lean_ai_proof": None},
        "axes": list(AXES),
        "stdlib": stdlib_metrics,
        "totals": _axis_totals(categories),
        "categories": categories,
    }


def _fmt_rate(rate: float | None) -> str:
    return STATUS_SKIP if rate is None else f"{rate:.2%}"


def _fmt_number(value: float | None) -> str:
    return STATUS_SKIP if value is None else f"{value:.4f}"


def _format_agent_runs(agent_runs: dict) -> list[str]:
    """Render the LLM provenance of agent-backed axes (empty when none)."""
    rows = []
    for axis, label in (
        ("repair_convergence", "repair convergence"),
        ("lean_ai_proof", "trust surface `lean_ai_proof.on`"),
    ):
        run = agent_runs.get(axis)
        if not run:
            continue
        budget = run.get("max_retries", run.get("lean_ai_proof_max_attempts"))
        rows.append(
            f"| {label} | `{run.get('llm_model', STATUS_SKIP)}` | "
            f"{_fmt_count(budget)} | `{run.get('schema', STATUS_SKIP)}` |"
        )
    if not rows:
        return []
    return [
        "### Agent Run Provenance",
        "",
        "| Axis | LLM | Attempt budget | Manifest schema |",
        "|------|-----|----------------|-----------------|",
        *rows,
        "",
    ]


def _fmt_count(value: int | None) -> str:
    return STATUS_SKIP if value is None else str(value)


def _fmt_counterexample_artifacts(block: dict | None) -> str:
    if not block or not block.get("files"):
        return "none"
    return (
        f"{_fmt_rate(block['as_expected_rate'])} as expected "
        f"({block['as_expected_emissions']}/{block['attempted_emissions']}), "
        f"{block['leaked_build_artifacts']} leaked build artifacts, "
        f"{block['refutation_certificates']}/{block['files']} refutation certificates"
    )


def _fmt_lean_ai_on(block: dict | None) -> str:
    if not block or block["on"]["status"] == STATUS_SKIP:
        return STATUS_SKIP
    on = block["on"]
    if on["status"] == STATUS_INCOMPLETE:
        stale = on.get("stale_files", [])
        stale_note = f", {len(stale)} stale" if stale else ""
        return (
            f"{STATUS_INCOMPLETE} ({on['files']} of "
            f"{on['files'] + len(on['missing_files'])} candidate files covered{stale_note}, no delta)"
        )
    if block["off"]["status"] == STATUS_INCOMPLETE:
        return (
            f"{_fmt_count(on['lean_verified_atoms'])} lean_verified, no delta "
            f"(off run {STATUS_INCOMPLETE}: {len(block['off']['unmeasured_files'])} candidate files not MEASURED)"
        )
    delta = block["lean_verified_delta"]
    return (
        f"{_fmt_count(on['lean_verified_atoms'])} lean_verified, "
        f"{'SKIP' if delta is None else f'{delta:+d}'} delta, "
        f"{_fmt_count(on['ai_proof_used_atoms'])} ai_proof_used, "
        f"{_fmt_count(on['manual_lemma_reason_remaining'])} manual_lemma_reason left"
    )


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
        *_format_agent_runs(evaluation.get("agent_runs") or {}),
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
            "Lean escalation candidates, "
            f"{_fmt_count(totals['trust_surface'].get('lean_verified_atoms'))} lean_verified "
            f"(AI off; AI on: {_fmt_lean_ai_on(totals['trust_surface'].get('lean_ai_proof'))})",
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
            f"/{totals['runtime_artifact_utility']['attempted_emissions']} emissions, "
            "expected-PASS tasks; counterexample tasks: "
            f"{_fmt_counterexample_artifacts(totals['runtime_artifact_utility'].get('counterexample'))})",
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
        (
            category["category"],
            entry["file"],
            target,
            "missing" if _expected_emission(entry.get("expected", "PASS"), target) else "leaked",
        )
        for category in evaluation["categories"]
        for entry in category["axes"]["runtime_artifact_utility"].get("files", [])
        for target in ARTIFACT_TARGETS
        if not entry["targets"][target].get(
            "as_expected", entry["targets"][target]["emitted"]
        )
    ]
    if gaps:
        lines.extend([
            "",
            "### Artifact Emission Gaps",
            "",
            "`missing`: an expected-PASS task yielded no artifact. `leaked`: a",
            "counterexample task left a non-empty build artifact behind despite the",
            "rejected verdict.",
            "",
            "| Category | File | Target | Gap |",
            "|----------|------|--------|-----|",
        ])
        lines.extend(
            f"| {category} | `{name}` | `{target}` | {kind} |"
            for category, name, target, kind in gaps
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
    parser.add_argument(
        "--ai-proof-cert-dir",
        type=Path,
        default=None,
        help=(
            "Directory of proof certificates produced with the mumei-agent AI "
            "Lean proof path enabled (B-7 'on' side); without it the on side is SKIP"
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
    ai_proof_certs = load_ai_proof_certificates(args.ai_proof_cert_dir)
    if not ai_proof_certs:
        print(
            "no AI-on proof certificates found; lean_ai_proof.on will be SKIP",
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
            ai_proof_certs=ai_proof_certs,
        )
        for name, dir_path in sorted(harness.CATEGORIES.items())
        if dir_path.is_dir()
    ]
    evaluation = build_evaluation(
        timestamp,
        categories,
        harness.collect_stdlib_metrics(),
        budget_policy_fingerprint=args.budget_policy_fingerprint,
        agent_runs={
            "repair_convergence": load_agent_run_manifest(args.repair_cert_dir),
            "lean_ai_proof": load_agent_run_manifest(args.ai_proof_cert_dir),
        },
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
