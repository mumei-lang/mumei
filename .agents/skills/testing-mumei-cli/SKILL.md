---
name: testing-mumei-cli
description: Test Mumei CLI verification, build, proof certificate, and polymorphic array flows locally. Use when validating Mumei language/runtime/compiler changes through the CLI.
---
# Testing Mumei CLI

## Devin Secrets Needed

None for local CLI verification/build/report testing.

## Prerequisites

### Build
```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo build
```
Binary is at `target/debug/mumei`.

### Z3 Solver
Mumei requires the `z3` binary on PATH for verification commands (`verify`, `build`, `verify-cert`, `publish`).
```bash
sudo apt-get install -y z3
```
If Z3 is missing, commands exit immediately with "Z3 solver not found" — no partial output.

### Rust Toolchain
- Rust 1.94+ required
- LLVM 17 with Polly support (`/usr/lib/llvm-17`)

## Quick Start

```bash
cd /home/ubuntu/repos/mumei
export LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu
cargo build
./target/debug/mumei verify std/contracts.mm --json        # smoke test
./target/debug/mumei verify --report-dir /tmp/r tests/test_contradiction.mm  # exits 1 by design (contradictory fixture); diagnostics -> /tmp/r/report.json
```

1. Build the branch binary (above) and run the focused Rust test for the area first, when one exists (`cargo test --test <name>`).
2. Pick the flow for the touched code from the table below and open its reference file.
3. Write temporary fixtures outside the repo (e.g. `/home/ubuntu/mumei-<topic>-fixtures/`), run the CLI, and assert on exit code, stdout/stderr and generated JSON.
4. Clean up: temporary fixtures, generated `cross_spec.json` / proof certs / reports in the repo, then check `git status`.

## Core Rules

- Use absolute `LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu` for every cargo/CLI command.
- `verify` exit codes: 0 verified, 1 rejected, 2 usage, 3 inconclusive/unverifiable, 4 input error, 5 internal error. Don't treat every nonzero code as rejection.
- Parse the entire `--json` stdout with `json.loads` and compare its `exit_code` with the real return code; prefer `--report-dir`/`report.json` for diagnostic assertions.
- Clear `.mumei_cache` / `.mumei_build_cache` / `.mumei` before each run when comparing binaries or re-running a fixture; warm caches print `skipped (unchanged, cached)`.
- Shell-only CLI flows need no browser recording; collect command output and generated JSON.
- Trust the path the CLI prints for generated artifacts (proof certs may land in the CWD).

## Which Flow to Use

| Change touches | Reference |
|---|---|
| A CLI flag/subcommand invocation, or choosing a fixture | [commands.md](references/commands.md) |
| `spurious_detection.rs`, `--disable-spurious-detection`, counterexample handling | [verification-flows.md](references/verification-flows.md) (P8-A) |
| `cross_spec/`, `cross_spec_verify`, module-level reports | [verification-flows.md](references/verification-flows.md) (Cross-spec report) |
| Contradictions, unsat cores, `report.json`, self-healing diagnostics | [verification-flows.md](references/verification-flows.md) |
| Array types, MIR element inference, Z3 array sorts, ArrayStore coercion | [verification-flows.md](references/verification-flows.md) (Polymorphic `[T]`) |
| Tuple returns, `**`/`Op::Pow`, chained comparisons, `unverifiable` verdict | [verification-flows.md](references/verification-flows.md) |
| `mir_analysis/borrow_check.rs` | [verification-flows.md](references/verification-flows.md) (Borrow checker) |
| `session_types.rs`, `--cross-spec-files`, `pipeline.rs` cross-spec loading | [session-types-and-monitors.md](references/session-types-and-monitors.md) |
| `mumei-emit-monitor/`, `trust_boundary.rs`, `--emit runtime-monitor` | [session-types-and-monitors.md](references/session-types-and-monitors.md) |
| Capability declarations (`parse_capability_def`, `Item::CapabilityDef`) | [session-types-and-monitors.md](references/session-types-and-monitors.md) |
| `emitter.rs`, external `--emit <name>` plugins | [certificates-emitters-lean.md](references/certificates-emitters-lean.md) |
| Escalation bundles, mumei-lean bridge schema, `--allow-lean-verified` | [certificates-emitters-lean.md](references/certificates-emitters-lean.md) |
| Proof-certificate generation, `verify-cert`, hashing, import caching | [certificates-emitters-lean.md](references/certificates-emitters-lean.md) |
| `otel` feature, `src/telemetry.rs`, TRACEPARENT | [otel-tracing.md](references/otel-tracing.md) |
| Exit codes, `--json` schema, benchmark verdict classification | [notes-exit-codes-json.md](references/notes-exit-codes-json.md) |
| `src/setup.rs`, `mumei setup`, Z3/LLVM pins in `release.yml` | [setup-installer.md](references/setup-installer.md) |

## Reference Files

- Read [references/commands.md](references/commands.md) when you need the full CLI command table or the list of existing test fixtures.
- Read [references/verification-flows.md](references/verification-flows.md) when testing Z3 verification behavior (spurious counterexamples, cross-spec reports, contradictions, arrays, tuples, `**`, `unverifiable`, borrow checker).
- Read [references/session-types-and-monitors.md](references/session-types-and-monitors.md) when testing session-type protocols, bounded session analysis, runtime monitors or capability declarations.
- Read [references/certificates-emitters-lean.md](references/certificates-emitters-lean.md) when testing external emitters, Lean escalation, Lean-verified certificates or proof certificates.
- Read [references/otel-tracing.md](references/otel-tracing.md) when testing OpenTelemetry/TRACEPARENT tracing.
- Read [references/notes-exit-codes-json.md](references/notes-exit-codes-json.md) when asserting exit codes or JSON output, or comparing two binaries.
- Read [references/setup-installer.md](references/setup-installer.md) when testing the `mumei setup` toolchain installer.
