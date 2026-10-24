# Claim Book

The Claim Book is a read-only report of which atom contracts are machine-checked, which rest on reviewed assumptions, and whether their evidence remains current after source edits. It reads existing proof certificates and current sources; it does not change verification results.

## CLI

```bash
mumei claims src/main.mm
mumei claims src/main.mm --cert src/main.proof.json --format json
mumei claims src/main.mm --allow-lean-verified --out-dir build/claims
```

The default format is `both`, writing `claimbook.json` and `CLAIMBOOK.md` in the current directory. `--format` accepts `json`, `md`, or `both`; `--out-dir` selects the output directory. The default certificate path is the same path used by `mumei verify --proof-cert`. Without a certificate, every current atom is `uncertified`. The command prints a short grade and freshness summary.

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

An integrity mismatch marks every claim as drifted. An integrity mismatch from a different generator version is shown as `incomparable_version` without changing grades on that basis. Certificate atoms absent from the source are listed in `orphaned_entries`. In this version, `effective_grade` is the own grade for fresh evidence and G0 otherwise.

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
    "evidence_kind": "z3",
    "freshness": "fresh",
    "reason": "Z3 discharged the obligation",
    "requires": "true",
    "ensures": "result == x + 1",
    "assumed_clauses": [],
    "dependencies": [],
    "content_hash": "...",
    "proof_hash": "..."
  }],
  "orphaned_entries": []
}
```
