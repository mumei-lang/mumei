# Claim Book

The Claim Book is a read-only report of which atom contracts are machine-checked, which rest on reviewed assumptions, and whether their evidence remains current after source edits. It reads existing proof certificates and current sources; it does not change verification results.

## CLI

```bash
mumei claims src/main.mm
mumei claims src/main.mm --cert src/main.proof.json --format json
mumei claims src/main.mm --allow-lean-verified --out-dir build/claims
mumei claims src/main.mm --require-grade G1
mumei claims src/main.mm --controls --max-mutants 16
mumei claims src/main.mm --require-discriminating
```

The default format is `both`, writing `claimbook.json` and `CLAIMBOOK.md` in the current directory. `--format` accepts `json`, `md`, or `both`; `--out-dir` selects the output directory. The default certificate path is the same path used by `mumei verify --proof-cert`. If that derived path is absent, every current atom is `uncertified`; an explicitly supplied but missing `--cert` path is an input error. The command prints a short grade and freshness summary.

`mumei claims` does not write the resolver cache, though manifest dependencies that have not been fetched yet may still be fetched into the dependency store.

## Negative controls

`--controls` adds per-claim negative-control results using the same mutation
engine as [`mumei controls`](CONTROLS.md). `--max-mutants N` sets the
per-atom limit (default 32) and is valid only with `--controls`. The
`--require-discriminating` gate enables controls automatically and exits 1 if
any claim is `weak` or `inconclusive`. `discriminating`, `no_mutants`, and
`not_checked` pass this gate; trusted atoms remain subject to the grade gate.

Controls are orthogonal to Seal Grade: enabling them does not change grades,
effective grades, freshness, or the Claim Book schema. JSON adds optional
`discrimination` fields on claims and `summary.by_discrimination`; Markdown
adds a Controls column and status-count summary. Both optional outputs are
omitted when `--controls` is not requested, preserving the existing report
bytes. Controls analysis remains read-only and writes no cache.

A `weak` result identifies a surviving mutant, not necessarily a real
specification weakness. The generator skips a small set of obvious equivalent
mutations, but other semantically equivalent mutants may survive and need
human judgment; see [Contract controls](CONTROLS.md).

## Seal Grade

| Grade | Label | Meaning |
|---|---|---|
| G5 | dual-sealed | Reserved; emitted only when its evidence sources exist. |
| G4 | machine-sealed | Closed by a machine checker. |
| G3 | range-sealed | Reserved; emitted only when its evidence sources exist. |
| G2 | observed | Reserved; emitted only when its evidence sources exist. |
| G1 | vouched | Rests on a reviewed assumption (`trusted`). |
| G0 | open | No current evidence. |

Z3-discharged obligations are G4. Lean evidence is G4 only when explicitly enabled, its translator metadata is current, and the axiom audit passed. A `trusted` source declaration is G1, and a `trusted` certificate entry is also G1. Other evidence is G0. An unsatisfiable specification is G0. Trust is read from the source declaration because certificates for trusted atoms record the solver result of the contract-only check.

## Freshness

- `fresh`: the source still matches its certificate; proof failures remain fresh evidence about the current source.
- `drifted`: the atom content or a recorded dependency contract changed, Lean translator metadata is stale, or a comparable certificate integrity mismatch was found.
- `uncertified`: the source atom has no certificate entry.

An integrity mismatch marks every claim as drifted. An integrity mismatch from a different generator version is shown as `incomparable_version` without changing grades on that basis. Certificate atoms absent from the source are listed in `orphaned_entries`.

When a dependency contract changed, the claim is `drifted`, its reason is
`a dependency's contract changed since the certificate was produced`, and its
own grade is capped at G0. Certificates without dependency hashes retain the
current freshness grade because the dependency check is unavailable.

## Effective grade and limiting premises

The base grade is the own grade for fresh evidence and G0 otherwise. If source or certificate assumed clauses are present, the base grade is capped at G1. The effective grade is the minimum of that base grade and the effective grades of dependencies present in the Claim Book. Grades are lowered by monotone fixpoint iteration, so mutually recursive dependencies settle at their minimum grade.

- A dependency resolves to an exact claim name first; otherwise a bare method name resolves only when exactly one claim ends in `::<name>`; these suffix matches propagate grade but mark completeness false because the match may be an imported free function, and their limiting premise identifies the method-name match.
- `assumed_clauses` is the sorted, deduplicated union of certificate values and source clauses declared with `assume` mode, rendered as `requires: ...` or `ensures: ...`.
- `unresolved_dependencies` lists only direct missing dependencies, and `effective_grade_complete` is false when any transitively reachable resolved dependency has unresolved dependencies; it is also false when a method-name match is reachable.

Dependencies not present in the claim set do not lower the grade. `limiting_premises` lists the freshness state, assumed clauses, and direct dependencies that lower the effective grade below the own grade. Each entry includes its kind, name, and grade; method-name dependency entries keep the qualified claim in `name` and record the bare name in `matched_from`. The list is empty when the effective grade equals the own grade. Markdown presents examples as `dependency b (G1); assumed_clause x > 0 (G1)`, or `dependency \`Point::get\` (matched from \`get\` by method name) (G1)`, with `-` when there are no limiting premises.

## Trusted base

The `trusted_base` section lists the solver and generator versions, translator versions, bridge lemma hashes, kernel axioms, trusted atoms, and assumed clauses represented by the report. Certificate-backed values come only from entries matching current source claims; source declarations contribute trusted atoms and assume-mode clauses. Assumed clauses retain the `requires:` or `ensures:` kind, so identical expressions of different kinds remain distinct. Lists are sorted and unique. Without a certificate, versions are null and the trusted atoms and assumed clauses are read from the source declarations.

## Gates and exit codes

`--require-grade G0` through `--require-grade G5` requires every claim to meet the requested effective grade. The report files are written before the gate is evaluated. A failing claim is printed to standard error with its limiting premises. The gate checks grade only; CI that needs full dependency coverage should also check `effective_grade_complete` in `claimbook.json`.

`--require-discriminating` can be combined with `--require-grade`; reports are
written before either gate is evaluated, and either failure produces exit 1.

| Exit code | Meaning |
|---|---|
| 0 | Reports written; every requested gate passes, or no gate was requested. |
| 1 | A claim is below `--require-grade`, or has weak/inconclusive controls when required. |
| 2 | Command-line arguments are invalid, including an unsupported grade. |
| 4 | Input or certificate could not be loaded. |
| 5 | An output artifact could not be written. |

## JSON

The deterministic JSON schema is `mumei.claimbook/v1`. A compact example:

```json
{
  "schema": "mumei.claimbook/v1",
  "input": "src/main.mm",
  "certificate": "src/main.proof.json",
  "certificate_integrity": "match",
  "generator": {
    "mumei_version": "0.6.22",
    "z3_version": "4.14.1"
  },
  "trusted_base": {
    "solver_version": "4.14.1",
    "mumei_version": "0.6.22",
    "translator_versions": [],
    "bridge_lemma_hashes": [],
    "kernel_axioms": [],
    "trusted_atoms": [],
    "assumed_clauses": []
  },
  "summary": {
    "by_effective_grade": {"G0": 0, "G1": 0, "G2": 0, "G3": 0, "G4": 1, "G5": 0},
    "by_freshness": {"fresh": 1, "drifted": 0, "uncertified": 0},
    "total": 1
  },
  "claims": [{
    "atom": "increment",
    "grade": "G4",
    "grade_label": "machine-sealed",
    "effective_grade": "G4",
    "effective_grade_label": "machine-sealed",
    "effective_grade_complete": true,
    "evidence_kind": "z3",
    "freshness": "fresh",
    "reason": "Z3 discharged the obligation",
    "requires": "true",
    "ensures": "result == x + 1",
    "assumed_clauses": [],
    "dependencies": [],
    "unresolved_dependencies": [],
    "limiting_premises": [],
    "content_hash": "...",
    "proof_hash": "..."
  }],
  "orphaned_entries": []
}
```
