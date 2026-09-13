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
```

### Exit codes

Only `0` and `1` are verdicts about the program. Every other code means the
verifier never judged the obligations, so a CI or benchmark harness should
treat it as "no verdict" rather than as a caught counterexample. `1` keeps its
historical meaning (rejection), so callers that only distinguish `0` / `1`
keep working.

| Code | Meaning | Typical cause |
|---|---|---|
| `0` | Verified | every obligation discharged (or delegated to an accepted certificate) |
| `1` | Rejected | Z3 counterexample, contract / type / session-protocol violation, strict array-type violation |
| `2` | Usage error | invalid command-line arguments (reported by the argument parser), including unsupported `--emit` / `--no-emit` targets |
| `3` | Inconclusive | no counterexample, but an obligation ended `unknown` / `timeout` / `resource_limit`, was reported `unverifiable` (unsupported Z3 clause), or is a `--escalate-lean` candidate Z3 left `unknown` that the Lean bridge did not discharge |
| `4` | Input error | the input file, a `--cross-spec-files` entry, or the directory could not be read, parsed, or resolved (missing file, unresolved import, empty directory) |
| `5` | Internal error | the verifier panicked, or an artifact / certificate / Lean-bridge step could not be completed |

For a directory run the process exit code is the most severe per-file outcome
(`5` > `4` > `1` > `3` > `0`); the per-file summary still lists each file's
own result. Rejected obligations are still counted in the `failed` field of the
printed summary and of `--json` output; only the exit code distinguishes a
counterexample from an inconclusive solver result.
