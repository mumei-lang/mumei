# Claim Book

The Claim Book is a read-only report of which atom contracts are machine-checked, which rest on reviewed assumptions, and whether their evidence remains current after source edits. It reads existing proof certificates and current sources; it does not change verification results.

## CLI

```bash
mumei claims src/main.mm
mumei claims src/main.mm --cert src/main.proof.json --format json
mumei claims src/main.mm --allow-lean-verified --out-dir build/claims
mumei claims src/main.mm --require-grade G1
mumei claims src/main.mm --observations build/test-harvest.json
mumei claims src/main.mm --controls --max-mutants 16
mumei claims src/main.mm --require-discriminating
mumei claims src/main.mm --require-grade G4 --require-complete
```

The default format is `both`, writing `claimbook.json` and `CLAIMBOOK.md` in the current directory. `--format` accepts `json`, `md`, or `both`; `--out-dir` selects the output directory. The default certificate path is the same path used by `mumei verify --proof-cert`. If that derived path is absent, every current atom is `uncertified`; an explicitly supplied but missing `--cert` path is an input error. The command prints a short grade and freshness summary.

`--observations` accepts the `mumei.test-harvest/v1` file consumed by
[`mumei observe`](CLI.md). Each observation is matched to its exact atom target,
including qualified method names such as `Struct::method`. `consistent` and
`entailed` support a claim; `contradicts_ensures` and `not_entailed` contradict
it; other verdicts are neutral. A claim is `contradicted` if any observation
contradicts it, `supported` if at least one supports it and none contradicts
it, `inconclusive` if it has only neutral observations, and `not_observed` if
there is no matching observation.

A `supported` observation lifts a claim whose current base grade is G0 (own
grade G0, or a non-fresh certificate) to G2 unless a fresh certificate records
an unsatisfiable specification. This uses observations checked against current
source, so certificate absence or drift does not block the lift; fresh G1 and
G4 claims do not change. The lift reason preserves the previous grade, evidence
kind, and reason. With the flag, JSON adds each claim's
`observation` (`status`, `supporting`, `contradicting`, and `neutral`) and
`summary.by_observation`; Markdown adds an Observation column and status-count
table. Without the flag, these fields and Markdown additions are omitted.

`mumei claims` does not write the resolver cache, though manifest dependencies that have not been fetched yet may still be fetched into the dependency store.

## Negative controls

`--controls` adds per-claim negative-control results using the same mutation
engine as [`mumei controls`](CONTROLS.md). `--max-mutants N` sets the
per-atom limit (default 32) and is valid only with `--controls`. The
`--require-discriminating` gate enables controls automatically and exits 1 if
any claim is `weak` or `inconclusive`. `discriminating`, `no_mutants`, and
`not_checked` pass this gate; trusted atoms remain subject to the grade gate.

Controls are orthogonal to Seal Grade: enabling them does not change grades,
effective grades, freshness, or observation results. JSON adds optional
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
| G5 | dual-sealed | Z3 returned `unsat` and a current Lean dual-check passed its axiom audit; requires `--allow-lean-verified`. Evidence: `z3+lean`. |
| G4 | machine-sealed | Closed by a machine checker. |
| G3 | range-sealed | Reserved until a bounded-proof evidence source exists. |
| G2 | observed | No machine proof; current test observations agree with the contract (`--observations`). |
| G1 | vouched | Rests on a reviewed assumption (`trusted`). |
| G0 | open | No current evidence. |

Z3-discharged obligations are G4 unless an explicitly enabled, current Lean dual-check also passes its axiom audit, which yields G5. Lean-only evidence remains G4. A `trusted` source declaration is G1, and a `trusted` certificate entry is also G1. Other evidence is G0. An unsatisfiable specification is G0. Trust is read from the source declaration because certificates for trusted atoms record the solver result of the contract-only check.

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

The base grade is the own grade for fresh evidence and G0 otherwise, except that observation-backed G2 remains G2 even when the certificate is absent or drifted. If source or certificate assumed clauses are present, the base grade is capped at G1, including for G2. The effective grade is the minimum of that base grade and the effective grades of dependencies present in the Claim Book. Grades are lowered by monotone fixpoint iteration, so mutually recursive dependencies settle at their minimum grade.

- A dependency resolves to an exact claim name first; otherwise a bare method name resolves only when exactly one claim ends in `::<name>`; these suffix matches propagate grade but mark completeness false because the match may be an imported free function, and their limiting premise identifies the method-name match.
- `assumed_clauses` is the sorted, deduplicated union of certificate values and source clauses declared with `assume` mode, rendered as `requires: ...` or `ensures: ...`.
- `unresolved_dependencies` lists only direct missing dependencies, and `effective_grade_complete` is false when any transitively reachable resolved dependency has unresolved dependencies; it is also false when a method-name match is reachable.

Dependencies not present in the claim set do not lower the grade. `limiting_premises` lists the freshness state, assumed clauses, and direct dependencies that lower the effective grade below the own grade. Each entry includes its kind, name, and grade; method-name dependency entries keep the qualified claim in `name` and record the bare name in `matched_from`. The list is empty when the effective grade equals the own grade. Markdown presents examples as `dependency b (G1); assumed_clause x > 0 (G1)`, or `dependency \`Point::get\` (matched from \`get\` by method name) (G1)`, with `-` when there are no limiting premises.

## Trusted base

The `trusted_base` section lists the solver and generator versions, translator versions, bridge lemma hashes, kernel axioms, trusted atoms, and assumed clauses represented by the report. Certificate-backed values come only from entries matching current source claims; source declarations contribute trusted atoms and assume-mode clauses. Assumed clauses retain the `requires:` or `ensures:` kind, so identical expressions of different kinds remain distinct. Lists are sorted and unique. Without a certificate, versions are null and the trusted atoms and assumed clauses are read from the source declarations.

## Gates and exit codes

`--require-grade G0` through `--require-grade G5` requires every claim to meet the requested effective grade. The report files are written before the gates are evaluated. A failing claim is printed to standard error with its limiting premises. The grade gate checks grade only.

`--require-complete` additionally requires at least one claim and every claim's
`effective_grade_complete` to be true. Coverage refers to dependencies recorded
in the Claim Book, not an additional analysis of all calls. It rejects direct or
transitive unresolved dependencies and method-name matches, even when the
effective grade meets the requested minimum. Diagnostics list incomplete claims
and their unresolved or incomplete dependencies; non-exact method-name resolution
is identified separately.
Complete dependency coverage does not establish proof, remove assumptions, or
verify external services. Combine this flag with a grade requirement when both
are needed. Unresolved built-ins or imported claims are not exempted by name.
Coverage remains scoped to each source's Claim Book; separate source reports
are not merged into a project-wide dependency closure.

The flag is opt-in and does not change report bytes, grade calculation, or
existing exit behavior when omitted. An empty Claim Book still succeeds without
this flag when the other requested gates pass.

`--require-discriminating`, `--require-complete`, and `--require-grade` can be
combined; reports are written before any gate is evaluated, and any requested
gate failure produces exit 1.

| Exit code | Meaning |
|---|---|
| 0 | Reports written; every requested gate passes, or no gate was requested. |
| 1 | A claim is below `--require-grade`, has weak/inconclusive controls when required, or fails required dependency coverage (including an empty Claim Book). |
| 2 | Command-line arguments are invalid, including an unsupported grade. |
| 4 | Input or certificate could not be loaded. |
| 5 | An output artifact could not be written. |

## Running the gate in CI

Call the reusable workflow from a repository workflow:

```yaml
name: Mumei gate

on: pull_request

jobs:
  gate:
    uses: mumei-lang/mumei/.github/workflows/mumei-gate.yml@develop
    with:
      working-directory: .
      sources: |
        src/main.mm
      require-grade: G4
      charter: true
```

Inputs:

| Input | Type | Default | Meaning |
|---|---|---|---|
| `mumei-version` | string | `""` | Empty downloads the latest release; otherwise downloads that release, accepting a version with or without a leading `v`. |
| `working-directory` | string | `"."` | Project directory containing the sources and optional `mumei.toml`. |
| `sources` | string | required | Whitespace-separated `.mm` paths relative to `working-directory`. |
| `require-grade` | string | `""` | Empty reports grades without gating; otherwise requires `G0` through `G5`. G5 also requires `allow-lean-verified: true` and matching upgraded `certificates`; stale supplied certificates are G0. |
| `allow-lean-verified` | boolean | `false` | Accept current Lean evidence for imports and Claim Book grades; required for G5. |
| `certificates` | string | `""` | Optional whitespace-separated proof certificates, one per source in the same order, relative to `working-directory`; Claim Book and Charter use these instead of the fresh certificate. |
| `require-discriminating` | boolean | `false` | Require the Claim Book negative-control gate to pass. |
| `require-complete` | boolean | `false` | Require a non-empty Claim Book with complete dependency coverage for each source. |
| `observations` | string | `""` | Test-harvest path relative to `working-directory`, passed to `mumei claims --observations`. |
| `charter` | boolean | `false` | Run `mumei charter check` using the `[charter]` configuration in `mumei.toml`. |
| `artifact-name` | string | `mumei-gate-reports` | Uploaded artifact name. |
| `runs-on` | string | `ubuntu-24.04` | Runner image; the GNU release binary needs glibc 2.39. |

Each run appends per-source command exit codes, Charter status, and the
`CLAIMBOOK.md` report to the GitHub Actions job summary. The
`mumei-gate-reports` artifact (or the configured artifact name) contains each
source's proof certificate and generated reports.

Set `require-complete: true` to enable the coverage gate. The Mumei binary must
include the `--require-complete` option; releases through v0.6.23 do not support
it. An older pinned binary fails explicitly rather than skipping the gate. The
workflow still runs the remaining enabled commands and uploads reports on a
gate failure. Leave the input false to preserve commands used by older releases.

`observations`, `allow-lean-verified`, and supplied `certificates` require a
Mumei release newer than v0.6.23.

G5 requires `allow-lean-verified: true` and `certificates` pointing at proof
certificates upgraded by mumei-lean's `bridge.py --dual-check` and present in
the checked-out repository. Fresh verification still runs for each source;
Claim Book and Charter use the supplied certificate instead. Claims checks
freshness against the current source, so a stale supplied certificate grades
G0, or G2 when `observations` support the claim. The gate rejects it only when
that grade is below `require-grade`, or, with `charter: true`, below G4, which
Charter requires when `require-grade` is empty. For example:

```yaml
with:
  allow-lean-verified: true
  certificates: src/main.proof.json
  require-grade: G5
```

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
