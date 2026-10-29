---
layout: default
title: "CLI Reference — Mumei"
description: "Mumei CLI commands for running, verifying, emitting LLVM IR or binaries, and executing proof-driven programs."
keywords: "mumei CLI, formal verification CLI, LLVM, Z3, mumei run"
---

# Mumei CLI

## `mumei run`

```bash
mumei run src/main.mm
mumei run src/main.mm --emit binary
mumei run src/main.mm --emit llvm-ir -o dist/app
```

`mumei run <file>` performs the native execution pipeline in one command:

1. parses and resolves the module, including imports and `mumei.toml` dependencies
2. verifies every atom with Z3
3. lowers all atoms into one LLVM module
4. exports `atom main()` as the C-compatible `main` entrypoint
5. compiles LLVM IR to an object file for `--emit binary`
6. links the object/IR with `clang` (falling back to `cc`/`gcc` for object files) and `runtime/mumei_runtime.c`
7. executes the resulting binary and returns its exit code

`atom main()` must be present and take no parameters. Its integer or floating-point result is converted to the process exit code. Runtime support includes channel helpers, named resource mutex lookup, and default effect-handler stubs for compiled `perform Effect.operation(...)` calls.

## `mumei verify`

```bash
mumei verify src/main.mm
mumei verify src/            # every .mm file under the directory
mumei verify src/main.mm --json --proof-cert
mumei verify src/main.mm --proof-store
mumei build src/main.mm --proof-store
```

Postcondition verification checks whether the requires and body constraints
form a reachable context. An unreachable context remains a contradiction
failure; the report classifies its ensures clauses as vacuous and includes a
diagnostic. Ensures entries in `report.json` include the context reachability
and an outcome for each non-trivial clause.

The opt-in `--enable-vacuity-check` flag runs Phase 1i: up to ten
deterministic, one-site mutants are checked through the real verifier. A
surviving mutant fails the atom; only structured `Rejected` verdicts count as
kills, while unknown or otherwise unanalyzable mutants do not. No-op `Pow`
mutations are skipped. This shares the site-mutant engine with
[`mumei controls`](CONTROLS.md), which provides a fuller per-atom report.

The local proof store is opt-in with `--proof-store` or `[proof] store = true`
in `mumei.toml`. `MUMEI_PROOF_STORE` selects its directory; otherwise it uses
`$HOME/.mumei/proof-store`. Setting the environment variable does not enable
the store. In `build`, `[proof] cache = false` disables it; an explicit
`--proof-store` then prints a warning. Only successful Z3 proofs are stored.
Trusted atoms, Lean-verified results, escalation candidates, failures, and
inconclusive results are never stored or reused.
On Unix, store directories are created with mode `0700` and entry files with
mode `0600`; existing group- or other-writable directories and entries are
refused. Store directories and entry files must be owned by the current
effective user; stores owned by another user are ignored with a warning.

### `mumei proof-store`

```bash
mumei proof-store stats [--dir <directory>] [--json]
mumei proof-store prune [--dir <directory>] [--older-than-days <N>] [--invalid]
```

`stats` reports the entry count, total bytes, and invalid-entry count.
`prune` removes entries selected by age and/or invalidity; at least one
selector is required (otherwise the command exits with code 4).

### Exit codes

Only `0` and `1` are verdicts about the program. Every other code means the
verifier never judged the obligations, so a CI or benchmark harness should
treat it as "no verdict" rather than as a caught counterexample. `1` keeps its
historical meaning (rejection), so callers that only distinguish `0` / `1`
keep working.

| Code | Meaning | Typical cause |
|---|---|---|
| `0` | Verified | every obligation discharged (or delegated to an accepted certificate) |
| `1` | Rejected | confirmed Z3 counterexample, contract / type / session-protocol violation, strict array-type violation |
| `2` | Usage error | invalid command-line arguments (reported by the argument parser), including unsupported `--emit` / `--no-emit` targets |
| `3` | Inconclusive | no confirmed counterexample, but an obligation ended `unknown` / `timeout` / `resource_limit`, Z3's model was classified a spurious candidate (`spurious_candidate`: the counterexample replay could not confirm it), the obligation was reported `unverifiable` (unsupported Z3 clause), or it is a `--escalate-lean` candidate Z3 left inconclusive that the Lean bridge did not discharge |
| `4` | Input error | the input file, a `--cross-spec-files` entry, or the directory could not be read, parsed, or resolved (missing file, unresolved import, empty directory) |
| `5` | Internal error | the verifier panicked, or an artifact / certificate / Lean-bridge step could not be completed |

For a directory run the process exit code is the most severe per-file outcome
(`5` > `4` > `1` > `3` > `0`); the per-file summary still lists each file's
own result. Rejected obligations are still counted in the `failed` field of the
printed summary and of `--json` output. The `--json` payload also carries the
outcome directly: `exit_code` is the process exit code from the table above,
`infra_errors` counts internal failures, and `status` is `passed` / `failed`
(rejected) / `unverifiable` / `inconclusive` / `input_error` / `internal_error`
in agreement with `exit_code` (a run whose only open items are Lean escalation
candidates or an infrastructure error reports the aggregate summary rather than
the single atom's `report.json`). A single-file run whose atoms all verified
prints the last atom's `report.json` instead of the summary; its `status` is the
per-atom `success` and it carries `exit_code: 0`. Runs that stop before any
atom is checked — unreadable input, an unresolved import, a failed
`--cross-spec-files` load, Z3 unavailable — still print one summary payload
with zero counts and the reason under `diagnostics`, so stdout is a single
JSON document for every single-file run that gets past argument parsing.
Usage errors (`2`) are reported by the argument parser on stderr and print no
JSON. `--emit loss-vector` / `--emit structured-feedback` print the emitted
artifact to stdout as a second JSON document ahead of the summary. A directory
run with `--json` prints one payload per file followed by the text summary;
parse it line-oriented or verify files individually.

## `mumei verify-cert`

`mumei verify-cert <certificate> <input>` checks certificate entries against
the current source. Trusted entries remain distinct from solver-proven entries
and do not fail `--strict`.

| Result | Meaning |
|---|---|
| `proven` | The source matches and the certificate records solver-proven evidence. |
| `trusted` | The source matches and the certificate records a trusted atom; its body was not checked. |
| `changed` | The source content hash differs from the certificate. |
| `dependency_changed` | A resolved dependency contract or effect definition differs from the certificate. |
| `dependency_unchecked` | One or more dependency hashes are absent or could not be resolved. |
| `stale_translator` | Lean translator metadata does not match the current version. |
| `unproven` | The source matches, but the certificate does not establish the contract. |
| `axiom_rejected` | The Lean axiom audit rejected the certificate evidence. |
| `missing` | The source contains no matching certificate entry. |

Dependency freshness includes resolved effect definitions under
`effect:<name>` keys, including definitions reached through `parent` and
`includes`.

The command exits 1 for a tampered certificate, `unproven`, `axiom_rejected`,
or `missing`. With `--strict`, it also exits 1 for `changed`,
`dependency_changed`, `dependency_unchecked`, or an absent
`certificate_hash`. It exits 0 otherwise.

## `mumei claims`

```bash
mumei claims <input.mm> [--cert <path>] [--format json|md|both]
  [--out-dir <directory>] [--controls] [--max-mutants <N>]
  [--observations <harvest.json>]
  [--require-discriminating] [--require-grade <G0..G5>]
```

`claims` writes a read-only Claim Book report. `--controls` attaches negative
control results to claims; `--max-mutants` sets the per-atom cap (default 32)
and requires `--controls`. `--require-discriminating` enables controls and
fails with exit 1 if any claim is weak or inconclusive. It may be combined
with `--require-grade`; reports are written before either gate is evaluated.
`--observations` accepts the `mumei.test-harvest/v1` JSON file used by
`mumei observe` and attaches its verdict counts to Claim Book claims. Supporting
observations can lift eligible G0 claims to G2; contradictions never lift a
grade. Controls remain orthogonal to grades. Without `--observations`, its
optional JSON and Markdown output is omitted.
Controls do not affect grades or freshness. Without `--controls`, JSON and
Markdown output are unchanged. Equivalent mutants beyond the generator's
small syntactic filter can still survive and require human judgment; see
[Contract controls](CONTROLS.md) and the [Claim Book guide](CLAIMBOOK.md).

## `mumei erosion`

```bash
mumei erosion <candidate.mm> --baseline <baseline.mm> [--json]
```

Compares the contracts in a candidate file with the same atoms in a baseline
file. A result distinguishes preserved, strengthened, and refined contracts
from missing or weakened contracts. The command checks top-level atoms and
implementation methods; newly added atoms are reported for information.
`--json` prints the deterministic `mumei.erosion-report/v1` report to standard
output. The command does not write report files.

Exit codes are `0` when no erosion is found, `1` when at least one baseline
contract is missing, mismatched, weakened, or inconclusive, and `4` for an
input or candidate-loading error. See [Contract Erosion](EROSION.md) for the
report schema and status definitions.

## `mumei charter`

```bash
mumei charter lock [--charter <file>]... [--lock <path>]
mumei charter check <impl.mm> [--charter <file>]... [--lock <path>] [--cert <path>] [--require-grade <G0..G5>] [--allow-lean-verified] [--json]
```

`charter lock` writes deterministic hashes for the selected Charter files.
`charter check` verifies the lock, checks that implementation atoms have the
same signatures as the Charter atoms, and proves that their contracts refine
the Charter contracts. With `--cert`, it also requires each otherwise-passing
atom to meet the certificate's effective Seal Grade, defaulting to G4.
`--require-grade` selects a threshold from G0 through G5, and
`--allow-lean-verified` accepts current Lean-verified certificate evidence.
The grade gate builds a read-only Claim Book from the current implementation
and certificate; incomplete dependency coverage is reported but does not by
itself fail the gate. When `mumei.toml` has a `[charter]` section, its
`files` and `lock` values supply defaults; the lock path defaults to
`charter.lock`. A check covers every Charter in the lock plus any files passed
with `--charter`. A Charter symlink that resolves outside the project is
rejected.

The check writes `charter_report.json`; `--json` also prints the report to
standard output. Its top-level `status` is `pass` or `fail`, and every atom
result identifies its relative Charter path. When a matching claim exists,
certificate-backed results include `effective_grade` and
`effective_grade_complete`. If the lock is mismatched, otherwise-passing
atoms are marked `unpinned` after the grade check. A passing check exits `0`,
a failed gate exits `1`, and an input or report I/O error exits `4`. See
[Charter](CHARTER.md) for the file format, lock behavior, and check semantics.

## `mumei controls`

```bash
mumei controls src/main.mm
mumei controls src/main.mm --json --atom Point::get
mumei controls src/main.mm --max-mutants 16
```

`controls` reports whether each verified atom contract rejects deterministic
one-site implementation mutations. It defaults to 32 mutants per atom and
supports an optional qualified atom filter. JSON reports use
`mumei.controls-report/v1`. Exit code `0` means no atom is weak, `1` means at
least one mutation survived, and `4` indicates missing or unparsable input or
an unknown atom. See [Contract controls](CONTROLS.md) for statuses, outcomes,
and the distinction from `verify --enable-vacuity-check`.

## `mumei observe`

```bash
mumei observe src/main.mm test-harvest.json
mumei observe src/main.mm test-harvest.json --json --atom Point::get
```

`observe` checks `mumei.test-harvest/v1` examples, relations, expected
rejections, and property-test observations against atom contracts using the
verifier. A property observation declares its generator `variables` (with
optional inclusive `min`/`max` bounds), `assumptions`, `args` that pin atom
parameters to literals or generator variables, and a `predicate` over those
variables and `result`.

JSON output uses `mumei.observation-check/v1`; each observation is `consistent`,
`contradicts_ensures`, `outside_requires`, `rejection_explained`,
`requires_admits_rejected`, `entailed`, `not_entailed`,
`domain_exceeds_requires`, `inconclusive`, or `not_checked`. Property checks
distinguish a contract that entails the predicate from one that does not, and
report when the generator domain extends beyond `requires`. Exit code `1`
indicates a contradiction or a rejected input admitted by `requires`;
`outside_requires` and `rejection_explained` never fail the run. For a relation,
`contradicts_ensures` means no result the contract allows satisfies the tested
predicate. Code `4` indicates invalid input or an unparsable source file.
`--atom` filters by the observation's target name.

### SARIF output

`mumei verify <file-or-directory> --emit sarif` writes a SARIF 2.1.0 log to
`<report-dir or .>/report.sarif`. A directory run contains the results from all
files in one log. Each result refers to an existing failure type, verification
outcome, or status; its `properties` always includes `atom` and `obligation`
(`requires`, `ensures`, `cover`, `atom`, `lean_proof`, `trait_law`, or
`session_protocol`), with `clause`,
`label`, `outcome`, `failure_type`, `z3_result`, `context_reachability`,
`counterexample`, `counterexample_fidelity`, and `witness` when available.
Session-protocol results also include `effect`, `protocol_state`,
`protocol_path`, `callee_atom`, and `suggested_fix`.
Covered cover clauses are represented as pass results (`kind: "pass"`,
`level: "none"`). `ruleIndex` refers to the matching entry in the run's rules.
A rule's `defaultConfiguration.level` is the most severe level among its
results (`error` > `warning` > `note` > `none`); rule order follows first use.

| Finding | SARIF rule ID | Level |
|---|---|---|
| Failed atom or postcondition | report failure type (or `failed`) | `error` |
| Spurious counterexample candidate | `spurious_candidate` | `warning` (review) |
| Vacuous, unknown, or skipped ensures clause | `vacuous`, `unknown`, or `skipped` | `warning` |
| Unverifiable atom | `unverifiable` | `warning` |
| Unknown cover result | `unknown` | `warning` |
| Covered cover result | `covered` | `none` (pass result) |
| Assumed clause | `assumed_clause` | `note` |
| Axiom-audit rejection | `axiom_rejected` | `warning` |
| Trait-law or impl error | `trait_law_violated` when a law has a counterexample; solver result when inconclusive; otherwise `failed` (e.g. a missing method or trait) | `error` (`warning` when inconclusive) |
| Session protocol violation | violation kind (`duality_mismatch`, `unreachable_receive`, `deadlock_no_progress`) | `error` |

The SARIF invocation records the combined process exit code (including all
files in a directory run); `executionSuccessful` is true for exit codes 0, 1,
and 3. Requesting SARIF does not change verification verdicts, the `--json`
payload, or the exit-code contract above.

### Preserving phase artifacts

`mumei verify <file-or-directory> --keep-phase-artifacts <DIR>` preserves each
atom's verification phases and the SMT-LIB queries issued during them:

```text
<DIR>/<source>-<hash8>/<atom>-<hash8>/phases.json
<DIR>/<source>-<hash8>/<atom>-<hash8>/<NNNN>-<phase-slug>.smt2
```

`<source>` is the atom's source path with every character outside
`[A-Za-z0-9._-]` replaced by `_`; `<atom>` uses the same sanitization (so
`S::m` becomes `S__m`). Each component is suffixed with `-<hash8>`, the first
eight lowercase hexadecimal characters of the SHA-256 hash of the original,
unsanitized value. The hash keeps distinct paths or atom names from colliding
when sanitization is lossy. An empty sanitized component becomes `_`. `NNNN`
is a 1-based query number for that atom, and the phase slug is the lower-case
phase name with non-alphanumeric runs replaced by `-`. Each `.smt2` file
contains comments for the atom, phase, and source origin, followed by the
solver text and its `(check-sat)` or `(check-sat-assuming (...))` command.

`phases.json` has `version: 1`, `atom`, `source_file`, `outcome`, `error`,
`partial`, and an ordered `phases` array. `partial` is `true` when a query
could not be captured; otherwise it is `false`. Each phase entry has `phase`,
`result`, and `queries`; each query has `index`, `file`, `origin`, and solver
`result` (`sat`, `unsat`, or `unknown`). Completed phases are included even
when they have no queries. Results may be `passed`, `failed`, `unknown`,
`unverifiable`, `contradiction`, or `aborted`. For an unchanged cached atom,
the file records `outcome: "cached"` and an empty `phases` array. An
unrecognized phase is retained verbatim and marked `in_phase_contract: false`.

Phase capture is diagnostic only: it does not change solver input, the
incremental-cache key, verification verdicts, `report.json`, `--json` output,
or exit codes. If artifact writes fail, one warning is printed to stderr after
verification, including with `--json`:
`warning: --keep-phase-artifacts: <n> artifact write(s) failed (first: <path>: <io error>); phase artifacts are incomplete`.
The failure never changes the verdict, report, JSON output, or exit code.
