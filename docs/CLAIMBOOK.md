# Claim Book

The Claim Book is a read-only report of which atom contracts are machine-checked, which rest on reviewed assumptions, and whether their evidence remains current after source edits. It reads existing proof certificates and current sources; it does not change verification results.

## CLI

```bash
mumei claims src/main.mm
mumei claims src/main.mm --cert src/main.proof.json --format json
mumei claims src/main.mm --allow-lean-verified --out-dir build/claims
mumei claims src/main.mm --require-grade G1
```

The default format is `both`, writing `claimbook.json` and `CLAIMBOOK.md` in the current directory. `--format` accepts `json`, `md`, or `both`; `--out-dir` selects the output directory. The default certificate path is the same path used by `mumei verify --proof-cert`. If that derived path is absent, every current atom is `uncertified`; an explicitly supplied but missing `--cert` path is an input error. The command prints a short grade and freshness summary.

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
- `drifted`: the atom content changed, Lean translator metadata is stale, or a comparable certificate integrity mismatch was found.
- `uncertified`: the source atom has no certificate entry.

An integrity mismatch marks every claim as drifted. An integrity mismatch from a different generator version is shown as `incomparable_version` without changing grades on that basis. Certificate atoms absent from the source are listed in `orphaned_entries`.

## Effective grade and limiting premises

The base grade is the own grade for fresh evidence and G0 otherwise. If the certificate lists assumed clauses, the base grade is capped at G1. The effective grade is the minimum of that base grade and the effective grades of dependencies present in the Claim Book. Grades are lowered by monotone fixpoint iteration, so mutually recursive dependencies settle at their minimum grade.

Dependencies not present in the claim set do not lower the grade. They are listed in sorted `unresolved_dependencies`, and `effective_grade_complete` is false when any are missing. `limiting_premises` lists the freshness state, assumed clauses, and direct dependencies that lower the effective grade below the own grade. Each entry includes its kind, name, and grade; the list is empty when the effective grade equals the own grade. Markdown presents examples as `dependency b (G1); assumed_clause x > 0 (G1)`, with `-` when there are no limiting premises.

## Trusted base

The `trusted_base` section lists the solver and generator versions, translator versions, bridge lemma hashes, kernel axioms, trusted atoms, and assumed clauses represented by the report. Lists are sorted and unique. Without a certificate, versions are null and the trusted atoms and assumed clauses are read from the source declarations.

## Grade gate and exit codes

`--require-grade G0` through `--require-grade G5` requires every claim to meet the requested effective grade. The report files are written before the gate is evaluated. A failing claim is printed to standard error with its limiting premises.

| Exit code | Meaning |
|---|---|
| 0 | Reports written; every claim meets the requested grade, or no gate was requested. |
| 1 | At least one claim is below `--require-grade`. |
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
