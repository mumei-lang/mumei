#!/usr/bin/env python3
"""Measure how far atom-local proof obligations compose into whole-system safety.

The script runs a *clause ablation* experiment over one or more `.mm` sources:

1. the source must verify as-is (baseline);
2. every `requires:` / `ensures:` clause is removed one at a time;
3. the resulting verification failures are attributed back to atoms.

Attribution decides whether an obligation is atom-local or a composition break:

* only the owning atom fails            -> ``atom_local`` (the clause discharges
  the atom's own obligation and nothing else depends on it);
* another atom fails                    -> ``composition_break`` (a neighbour
  cannot close its proof unless this contract is *stronger* than the owning
  atom needs locally);
* nothing fails                         -> ``unconstrained`` (contract slack).

``atom_local_closure_ratio`` is the share of load-bearing obligations that stay
atom-local, i.e. ``atom_local / (atom_local + composition_break)``.

The ablation also shows *how* atoms compose: because removing a callee's
`ensures` breaks its callers while the callee itself still verifies,
verification is contract-modular — a caller closes from the callee's declared
contract, never from the callee's body.  ``whole_system_invariants`` therefore
records the top-level atoms (the ones no other atom calls, i.e. the whole-cycle
invariants).  ``whole_system_invariants_closed`` counts those that close from
declared atom-local contracts alone (no global lemma, no whole-program
reasoning); ``whole_system_invariants_neighbor_dependent`` counts how many of
them lose that closure when a *neighbour* contract is weakened, which is the
surface a modular verifier has to keep stable.

Break patterns are grouped into ``modular_verification_inputs`` so they can be
read as input for `mumei-core` modular verification (`effect_pre` /
`effect_post`, Plan 24).

The emitted JSON is a measurement artifact.  It does not introduce audit or
verdict vocabulary: harness keys (`harness_contract`, `intent_fidelity`,
`artifact_paths`, `budget_policy_fingerprint`, `lean_verified`) and the no-`.mm`
keys keep their canonical meaning and are never re-spelled here.
"""
from __future__ import annotations

import argparse
import concurrent.futures
import json
import re
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import asdict, dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BIN = REPO_ROOT / "target" / "release" / "mumei"
FALLBACK_BIN = REPO_ROOT / "target" / "debug" / "mumei"

ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
ATOM_START = re.compile(r"^atom\s+([A-Za-z_][A-Za-z0-9_]*)")
ITEM_START = re.compile(r"^(atom|type|effect|import|resource)\b")
CLAUSE_START = re.compile(r"^\s*(requires|ensures|effect_pre|effect_post):")
ERROR_SPLIT = re.compile(r"Verification Error:")
SPAN = re.compile(r"╭─\[(?P<file>[^\]]+?):(?P<line>\d+):(?P<col>\d+)\]")
ATOM_IN_MESSAGE = re.compile(r"for atom '([A-Za-z_][A-Za-z0-9_]*)'")
CALL_IN_MESSAGE = re.compile(r"Call to '([A-Za-z_][A-Za-z0-9_]*)'")

# Break patterns are keyed by the compiler surface they feed back into.
PATTERN_HOOKS = {
    "neighbor_ensures_strengthening": "value contracts (`ensures`) of called atoms",
    "call_site_precondition": "call-site `requires` propagation",
    "effect_state_obligation": "`effect_pre` / `effect_post` state chaining (Plan 24)",
    "counterexample_replay_mismatch": "Z3 translation / Lean escalation path",
}


@dataclass
class Clause:
    atom: str
    kind: str
    line: int
    text: str


@dataclass
class BreakRecord:
    atom: str
    clause_kind: str
    clause_line: int
    clause_text: str
    affected_atoms: list[str]
    pattern: str
    diagnostic: str


@dataclass
class CaseResult:
    case: str
    source: str
    atom_count: int
    max_dependency_depth: int
    top_level_atoms: list[str]
    whole_system_invariants_closed: int
    whole_system_invariants_neighbor_dependent: int
    probed_clauses: int
    atom_local_obligations: int
    composition_breaks: int
    unconstrained_clauses: int
    atom_local_closure_ratio: float
    wall_clock_seconds: float
    breaks: list[BreakRecord] = field(default_factory=list)


def strip_ansi(text: str) -> str:
    return ANSI.sub("", text)


def parse_atoms(lines: list[str]) -> dict[str, tuple[int, int]]:
    """Map atom name -> (first line, last line), 1-based inclusive."""
    spans: dict[str, tuple[int, int]] = {}
    current: str | None = None
    start = 0
    for index, line in enumerate(lines, 1):
        match = ATOM_START.match(line)
        if match:
            if current is not None:
                spans[current] = (start, index - 1)
            current = match.group(1)
            start = index
        elif current is not None and ITEM_START.match(line):
            spans[current] = (start, index - 1)
            current = None
    if current is not None:
        spans[current] = (start, len(lines))
    return spans


def parse_clauses(lines: list[str], atoms: dict[str, tuple[int, int]]) -> list[Clause]:
    owner_of: dict[int, str] = {}
    for name, (start, end) in atoms.items():
        for line_number in range(start, end + 1):
            owner_of[line_number] = name

    clauses: list[Clause] = []
    index = 1
    while index <= len(lines):
        line = lines[index - 1]
        match = CLAUSE_START.match(line)
        if not match:
            index += 1
            continue
        end = index
        while end <= len(lines) and not lines[end - 1].rstrip().endswith(";"):
            end += 1
        owner = owner_of.get(index)
        if owner is not None:
            clauses.append(
                Clause(
                    atom=owner,
                    kind=match.group(1),
                    line=index,
                    text=" ".join(part.strip() for part in lines[index - 1 : end]),
                )
            )
        index = end + 1
    return clauses


def call_graph(lines: list[str], atoms: dict[str, tuple[int, int]]) -> dict[str, set[str]]:
    names = set(atoms)
    calls: dict[str, set[str]] = {name: set() for name in names}
    for name, (start, end) in atoms.items():
        body = "\n".join(lines[start - 1 : end])
        for callee in re.findall(r"([A-Za-z_][A-Za-z0-9_]*)\s*\(", body):
            if callee in names and callee != name:
                calls[name].add(callee)
    return calls


def top_level_atoms(calls: dict[str, set[str]]) -> list[str]:
    called = {callee for callees in calls.values() for callee in callees}
    return sorted(name for name in calls if name not in called)


def dependency_depth(calls: dict[str, set[str]]) -> int:
    memo: dict[str, int] = {}

    def depth(name: str, seen: frozenset[str]) -> int:
        if name in memo:
            return memo[name]
        if name in seen:
            return 0
        best = 0
        for callee in calls[name]:
            best = max(best, 1 + depth(callee, seen | {name}))
        memo[name] = best
        return best

    return max((depth(name, frozenset()) for name in calls), default=0)


def run_verify(binary: Path, source: Path, workdir: Path, timeout: int) -> str:
    process = subprocess.run(
        [str(binary), "verify", str(source), "--cache-scope", "module"],
        cwd=workdir,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    return strip_ansi(process.stdout + process.stderr)


def failing_atoms(output: str, atoms: dict[str, tuple[int, int]]) -> list[tuple[str, str]]:
    """Return (atom, diagnostic) pairs for each reported verification error."""
    found: list[tuple[str, str]] = []
    blocks = ERROR_SPLIT.split(output)[1:]
    for block in blocks:
        message = block.strip().splitlines()[0].strip() if block.strip() else ""
        named = ATOM_IN_MESSAGE.search(block)
        atom: str | None = named.group(1) if named else None
        if atom is None:
            span = SPAN.search(block)
            if span is not None:
                line = int(span.group("line"))
                for name, (start, end) in atoms.items():
                    if start <= line <= end:
                        atom = name
                        break
        if atom is None:
            atom = "<unattributed>"
        found.append((atom, message))
    return found


def classify(clause: Clause, diagnostic: str) -> str:
    lowered = diagnostic.lower()
    if "prestate" in lowered or ("effect" in lowered and "state" in lowered):
        return "effect_state_obligation"
    if clause.kind in {"effect_pre", "effect_post"}:
        return "effect_state_obligation"
    if "spurious counterexample" in lowered or "replay" in lowered:
        return "counterexample_replay_mismatch"
    if "precondition" in lowered or "call site" in lowered:
        return "call_site_precondition"
    return "neighbor_ensures_strengthening"


def probe_clause(
    binary: Path,
    source: Path,
    lines: list[str],
    atoms: dict[str, tuple[int, int]],
    clause: Clause,
    timeout: int,
) -> tuple[Clause, list[tuple[str, str]]]:
    end = clause.line
    while end <= len(lines) and not lines[end - 1].rstrip().endswith(";"):
        end += 1
    variant = lines[: clause.line - 1] + lines[end:]
    workdir = Path(tempfile.mkdtemp(prefix="mumei-composability-"))
    try:
        probe_source = workdir / source.name
        probe_source.write_text("\n".join(variant) + "\n", encoding="utf-8")
        output = run_verify(binary, probe_source, workdir, timeout)
        return clause, failing_atoms(output, atoms)
    finally:
        shutil.rmtree(workdir, ignore_errors=True)


def measure_case(
    binary: Path,
    source: Path,
    kinds: set[str],
    jobs: int,
    timeout: int,
) -> CaseResult:
    lines = source.read_text(encoding="utf-8").splitlines()
    atoms = parse_atoms(lines)
    calls = call_graph(lines, atoms)
    roots = top_level_atoms(calls)
    clauses = [clause for clause in parse_clauses(lines, atoms) if clause.kind in kinds]

    workdir = Path(tempfile.mkdtemp(prefix="mumei-composability-base-"))
    try:
        baseline_source = workdir / source.name
        baseline_source.write_text("\n".join(lines) + "\n", encoding="utf-8")
        baseline = run_verify(binary, baseline_source, workdir, timeout)
    finally:
        shutil.rmtree(workdir, ignore_errors=True)
    if "Verification passed" not in baseline:
        raise SystemExit(f"baseline verification failed for {source}")

    started = time.monotonic()
    local = 0
    unconstrained = 0
    breaks: list[BreakRecord] = []

    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = [
            pool.submit(probe_clause, binary, source, lines, atoms, clause, timeout)
            for clause in clauses
        ]
        for future in concurrent.futures.as_completed(futures):
            clause, failures = future.result()
            if not failures:
                unconstrained += 1
                continue
            others = sorted({atom for atom, _ in failures if atom != clause.atom})
            if not others:
                local += 1
                continue
            diagnostic = next(
                (message for atom, message in failures if atom != clause.atom), ""
            )
            breaks.append(
                BreakRecord(
                    atom=clause.atom,
                    clause_kind=clause.kind,
                    clause_line=clause.line,
                    clause_text=clause.text,
                    affected_atoms=others,
                    pattern=classify(clause, diagnostic),
                    diagnostic=diagnostic,
                )
            )

    breaks.sort(key=lambda record: (record.atom, record.clause_line))
    load_bearing = local + len(breaks)
    ratio = (local / load_bearing) if load_bearing else 1.0
    broken_roots = {
        atom for record in breaks for atom in record.affected_atoms if atom in roots
    }
    return CaseResult(
        case=source.parent.name,
        source=f"{source.parent.name}/{source.name}",
        atom_count=len(atoms),
        max_dependency_depth=dependency_depth(calls),
        top_level_atoms=roots,
        whole_system_invariants_closed=len(roots),
        whole_system_invariants_neighbor_dependent=len(broken_roots),
        probed_clauses=len(clauses),
        atom_local_obligations=local,
        composition_breaks=len(breaks),
        unconstrained_clauses=unconstrained,
        atom_local_closure_ratio=round(ratio, 4),
        wall_clock_seconds=round(time.monotonic() - started, 2),
        breaks=breaks,
    )


def modular_verification_inputs(cases: list[CaseResult]) -> dict[str, dict[str, object]]:
    grouped: dict[str, dict[str, object]] = {}
    for case in cases:
        for record in case.breaks:
            entry = grouped.setdefault(
                record.pattern,
                {
                    "count": 0,
                    "compiler_surface": PATTERN_HOOKS.get(record.pattern, "unclassified"),
                    "examples": [],
                },
            )
            entry["count"] = int(entry["count"]) + 1
            examples = entry["examples"]
            assert isinstance(examples, list)
            if len(examples) < 5:
                examples.append(
                    f"{case.case}:{record.atom}:{record.clause_kind}@{record.clause_line}"
                )
    return grouped


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sources", nargs="+", type=Path, help=".mm sources to measure")
    parser.add_argument("--mumei-bin", type=Path, default=None)
    parser.add_argument("--output", type=Path, default=None)
    parser.add_argument(
        "--clause-kinds",
        default="ensures",
        help="comma separated: requires,ensures,effect_pre,effect_post or 'all'",
    )
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument(
        "--budget-policy-fingerprint",
        default=None,
        help="fingerprint recorded alongside the measurement, mirroring proof certificates",
    )
    args = parser.parse_args()

    binary = args.mumei_bin
    if binary is None:
        binary = DEFAULT_BIN if DEFAULT_BIN.exists() else FALLBACK_BIN
    if not binary.exists():
        raise SystemExit(f"mumei binary not found: {binary}")

    if args.clause_kinds == "all":
        kinds = {"requires", "ensures", "effect_pre", "effect_post"}
    else:
        kinds = {kind.strip() for kind in args.clause_kinds.split(",") if kind.strip()}

    cases = [
        measure_case(binary, source.resolve(), kinds, args.jobs, args.timeout)
        for source in args.sources
    ]

    total_local = sum(case.atom_local_obligations for case in cases)
    total_breaks = sum(case.composition_breaks for case in cases)
    load_bearing = total_local + total_breaks
    report = {
        "schema": "mumei.composability.v1",
        "generated_by": "scripts/measure_composability.py",
        "clause_kinds": sorted(kinds),
        "budget_policy_fingerprint": args.budget_policy_fingerprint,
        "cases": [asdict(case) for case in cases],
        "totals": {
            "atom_count": sum(case.atom_count for case in cases),
            "whole_system_invariants": sum(len(case.top_level_atoms) for case in cases),
            "whole_system_invariants_closed": sum(
                case.whole_system_invariants_closed for case in cases
            ),
            "whole_system_invariants_neighbor_dependent": sum(
                case.whole_system_invariants_neighbor_dependent for case in cases
            ),
            "probed_clauses": sum(case.probed_clauses for case in cases),
            "atom_local_obligations": total_local,
            "composition_breaks": total_breaks,
            "unconstrained_clauses": sum(case.unconstrained_clauses for case in cases),
            "atom_local_closure_ratio": round(
                (total_local / load_bearing) if load_bearing else 1.0, 4
            ),
        },
        "modular_verification_inputs": modular_verification_inputs(cases),
    }

    text = json.dumps(report, indent=2, ensure_ascii=False, sort_keys=True)
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text + "\n", encoding="utf-8")
        print(f"composability report written to {args.output}")
    else:
        print(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
