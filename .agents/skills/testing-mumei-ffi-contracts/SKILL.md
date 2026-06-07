---
name: testing-mumei-ffi-contracts
description: Test Mumei std FFI contract strengthening through generated Rust property tests, CLI verification, and stdlib metrics. Use when changes touch std/file.mm, std/http*.mm, std/json.mm, scripts/ffi_contract_test_gen.py, or mumei-ffi-tests.
---
# Testing Mumei FFI Contracts

## Devin Secrets Needed

None for local FFI contract validation.

## Prerequisites

Use the repo's standard LLVM/Z3 environment variables for Rust and CLI commands:

```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17
LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu
MUMEI_STD_PATH=/home/ubuntu/repos/mumei/std
```

Build the CLI first if `target/debug/mumei` is missing or stale:

```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  cargo build --manifest-path /home/ubuntu/repos/mumei/Cargo.toml
```

## Primary FFI Contract Flow

Run the generated FFI property tests against the real Rust backend:

```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  cargo test --manifest-path /home/ubuntu/repos/mumei/Cargo.toml \
  -p mumei-ffi-tests --test ffi_contracts
```

Expected assertions:

- Command exits zero.
- Output includes `running 48 tests` or the current expected generated-test count if contracts were intentionally added/removed.
- Output includes `test result: ok.` with zero failures.
- Representative tests for changed areas are present and pass, e.g. file write/delete, HTTP status/header, HTTPS request, HTTP server response, and JSON bool/free contracts.

## CLI Verification Flow

Verify the changed std FFI modules through the Mumei CLI so extern declarations and Z3-facing contracts are loaded:

```bash
for module in file http http_secure http_server json; do
  MUMEI_STD_PATH=/home/ubuntu/repos/mumei/std \
    /home/ubuntu/repos/mumei/target/debug/mumei verify \
    "/home/ubuntu/repos/mumei/std/${module}.mm"
done
```

Expected assertions:

- Each command exits zero.
- Output includes the expected extern registration count for each module:
  - `file`: `registered 4 extern function(s)`
  - `http`: `registered 12 extern function(s)`
  - `http_secure`: `registered 8 extern function(s)`
  - `http_server`: `registered 8 extern function(s)`
  - `json`: `registered 20 extern function(s)`
- Each module prints `Verification passed` and `0 Lean escalation candidate(s)` unless the change intentionally introduces escalation candidates.

## Metrics Acceptance Flow

Regenerate stdlib metrics with history enabled unless a faster local smoke check is explicitly sufficient:

```bash
MUMEI_STD_PATH=/home/ubuntu/repos/mumei/std \
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  python3 /home/ubuntu/repos/mumei/scripts/generate_stdlib_metrics.py \
  --mumei-bin /home/ubuntu/repos/mumei/target/debug/mumei \
  --output /home/ubuntu/repos/mumei/docs/STDLIB_METRICS.md
```

Expected assertions for trusted-reduction work:

- Summary health meets the task target, e.g. `Weighted health score` is at least `0.980`.
- Total trusted atoms meet the requested budget, e.g. `Trusted atoms` is `<= 20`.
- Target FFI modules show `Trusted = 0` when they were intended to be fully converted (`std/file.mm`, `std/http.mm`, `std/http_secure.mm`, `std/json.mm`).
- Any remaining trusted atoms are expected and documented, such as stateful temporal-effect atoms in `std/http_server.mm` or unrelated trusted atoms already on the base branch.

## Cleanup

Mumei CLI verification can write `cross_spec.json` in the current working directory. Remove generated artifacts before final status checks:

```bash
rm -f /home/ubuntu/repos/mumei/cross_spec.json
```

If metrics were regenerated only for testing and the timestamp is the only diff, restore or commit intentionally so `git status --short` is clean before reporting.

## Recording

This flow is shell-only. Do not start a desktop/browser recording; collect command output as text evidence instead.
