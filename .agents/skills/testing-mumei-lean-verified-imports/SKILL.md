---
name: testing-mumei-lean-verified-imports
description: Test Mumei `--allow-lean-verified` imported proof-certificate flows through CLI run/publish. Use when changes touch proof certificate trust policy, import resolution, `cmd_run`, `cmd_publish`, or Lean-verified certificate acceptance.
---
# Testing Mumei Lean-Verified Imports

## Devin Secrets Needed

None for local CLI/runtime testing.

## Prerequisites

Build the CLI from the repo root before testing:

```bash
cd /home/ubuntu/repos/mumei
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build
```

The test requires LLVM 17, libclang, Z3, and the local `target/debug/mumei` binary. No browser recording is useful for this shell-only flow; collect stdout/stderr/status and generated proof-certificate paths as text evidence.

## Test Fixture Shape

Use a small imported package whose atoms have proof certificates transformed to `z3_check_result = "lean_verified"` and `lean_metadata.status = "lean_verified"`. The importing project should call those imported atoms from a simple `main`/publish entry atom.

Prefer deterministic, low-noise bodies such as:

- imported `safe_add` and `safe_double` atoms from `examples/import_test/lib/math_utils.mm`, or equivalent local fixtures.
- a native `run` entrypoint returning a stable non-zero value such as `15`, so the shell status proves native execution propagated the Mumei `main` result.
- a publish entrypoint with simple arithmetic and `ensures` that should verify without spurious counterexamples.

Avoid adversarial fixtures for this flow unless intentionally testing verifier failure modes; complex arithmetic/postconditions can obscure whether the import trust-policy path is working.

## `run --allow-lean-verified` Flow

Run the importing project with the flag:

```bash
cd /home/ubuntu/repos/mumei
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  target/debug/mumei run --allow-lean-verified /path/to/importing/main.mm \
  > /tmp/mumei-lean-run.out 2> /tmp/mumei-lean-run.err
status=$?
printf 'status=%s\n' "$status"
cat /tmp/mumei-lean-run.out
cat /tmp/mumei-lean-run.err
```

Expected assertions:

- Command status matches the Mumei `main` return value, e.g. exactly `15` for a `main` returning `15`.
- stderr includes `Lean-verified atom '<imported_atom>' accepted as proven (--allow-lean-verified)` for each imported Lean-verified atom used by the run.
- stdout includes `Mumei Run: verify → codegen → link → execute`.
- stdout includes `Verified: main` and `Running`.
- stderr does not include `Import Resolution Failed`, `Codegen failed`, `Linking failed`, or `undefined reference`.

## `publish --allow-lean-verified` Flow

Run publish from a temporary Mumei package/manifest that imports the Lean-verified package:

```bash
cd /path/to/publish-project
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  /home/ubuntu/repos/mumei/target/debug/mumei publish --allow-lean-verified \
  > /tmp/mumei-lean-publish.out 2> /tmp/mumei-lean-publish.err
status=$?
printf 'status=%s\n' "$status"
cat /tmp/mumei-lean-publish.out
cat /tmp/mumei-lean-publish.err
```

Expected assertions:

- Command exits zero.
- stderr includes `Lean-verified atom '<imported_atom>' accepted as proven (--allow-lean-verified)` for each imported Lean-verified atom used by publish verification.
- stdout includes `All 1 atom(s) verified.` or the expected atom count for the fixture.
- stdout includes `Proof certificate saved`.
- stdout includes `Published <package> <version> to local registry`.
- The published package proof certificate exists under `~/.mumei/packages/<package>/<version>/proof_certificate.json`.

## Supporting Regression Checks

When validating related TODO-debt or trust-policy work, also run focused Rust checks for identifier-boundary and nested method substitution behavior if touched:

```bash
cd /home/ubuntu/repos/mumei
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  cargo test -p mumei-core substitute_method_calls -- --nocapture
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  cargo check
```

Expected assertions:

- Nested method calls expand with enough dynamic passes.
- Already parenthesized nested arguments are not double-parenthesized.
- Method-name/diagnostic matching treats `_` as an identifier character and avoids partial matches.
- `cargo check` exits zero.

## Cleanup

- Keep temporary fixtures outside the repo, e.g. under `/home/ubuntu/mumei-lean-verified-*`.
- Delete generated `cross_spec.json`, proof certs, and temp package directories before final `git status`.
- Final `git status --short` in `/home/ubuntu/repos/mumei` should be clean.
