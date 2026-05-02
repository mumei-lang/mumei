# Testing Mumei CLI

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

## Key CLI Commands for Testing

| Command | Purpose |
|---|---|
| `mumei verify <file.mm>` | Verify atoms with Z3 |
| `mumei verify --proof-cert <file.mm>` | Verify + generate `.proof.json` certificate |
| `mumei verify-cert <cert.json> <file.mm>` | Verify cert against current source |
| `mumei build <file.mm>` | Build (verify + compile to LLVM IR) |
| `mumei build --emit proof-cert <file.mm>` | Note: `--emit proof-cert` dispatches to `ProofCert` emit target but returns empty artifacts. Use `verify --proof-cert` instead for cert generation. |
| `mumei build --emit <unknown_name> <file.mm>` | Phase 3 (Task 1-C): unknown emit targets fall through to `emitter::load_external_emitter`, which probes `~/.mumei/emitters/<name>/libmumei_emit_<name>.{so,dylib,dll}` (no `lib` prefix on Windows — matches Rust `cdylib` convention). When absent, exits 1 with two stderr lines: `❌ Error: Unknown emit target …` plus `External plugin lookup failed: External emitter '<name>' not found.` referencing the resolved path and the plugin contract symbol `mumei_create_emitter`. |
| `mumei publish` | Publish current project to local registry (requires `mumei.toml`) |
| `mumei add <pkg>` | Add dependency from registry (requires `mumei.toml`) |
| `mumei verify --strict-imports <file.mm>` | Strict import mode |
| `mumei build --strict-imports <file.mm>` | Strict import mode for build |
| `mumei verify --allow-lean-verified <file.mm>` | Accept mumei-lean-emitted certs (`z3_check_result == "lean_verified"`) as proven. When this flag triggers acceptance, the resolver audit-logs `🔗 Lean-verified atom '<name>' accepted as proven (--allow-lean-verified)` to stderr — useful as a grep target for tests. |

## Test Files

- `sword_test.mm` — 8 atoms covering loops, floats, stack ops. Note: `scale` atom fails verification (float multiplication precision), so `all_verified` will be `false`.
- `examples/import_test/main.mm` — Imports `lib/math_utils.mm`, good for import/dependency testing.
- `examples/import_test/lib/math_utils.mm` — 2 simple atoms (`safe_add`, `safe_double`), all verify successfully. Good for generating clean certificates.
- `tests/test_cross_atom_chain.mm` — Effect system + chained atom composition.
- `tests/test_nested_while_no_trusted.mm` — Regression for `Expr::ArrayAccess => i64` MIR inference (`let key = arr[i]` inside nested `while`). A regression in `mir.rs::infer_hir_ty()` would fire `UseAfterMove` here.
- `tests/test_verified_sort.mm` — Mirrors `std/list.mm::verified_insertion_sort` without `trusted`; same regression target as above plus the `forall(i, 0, n, arr[i] >= 0)` Z3-bounds idiom.

## Testing Flows

### Certificate Generation + Verification (P5-A)
1. `mumei verify --proof-cert <file.mm>` generates `<stem>.proof.json`
2. `mumei verify-cert <cert.json> <file.mm>` checks cert against source
3. Modify source, re-run verify-cert to see "changed" status

### Publish + Add (P5-B)
1. Create project dir with `mumei.toml` (needs `[package]` with name/version and `[build]` with entry)
2. `mumei publish` from project dir
3. Check `~/.mumei/registry.json` for `cert_path`/`cert_hash`
4. Create consumer project, `mumei add <pkg-name>` to resolve from registry

### Project Setup for Publish Testing
```toml
# mumei.toml
[package]
name = "test-pkg"
version = "0.1.0"
description = "Test"

[build]
entry = "main.mm"
```

## Known Quirks

- **`--strict-imports` on direct file imports**: For `import "./path.mm"`, the resolver uses legacy behavior (trust atoms) when no cert exists, even with `--strict-imports`. Strict enforcement only applies at manifest dependency level.
- **`verify-cert` exit codes**: "changed" atoms produce exit 0 (soft warning). Only "unproven"/"missing" atoms cause exit 1.
- **Verification caching — TWO locations**: To force a clean re-verify you need to clear *both* `.mumei/cache/verification_cache.json` (the enhanced cache, written/read by `resolver::{load,save}_verification_cache`) and `<input-dir>/.mumei_cache` (the legacy resolver cache, written by `resolve_imports_with_full_options`). After only clearing one, verify still reports `0 verified, N skipped (unchanged) ⚡`. Quick reset:
  ```bash
  rm -rf .mumei .mumei_build_cache <input-dir>/.mumei_cache
  ```
- **`-o` is a base name, not a full path**: `mumei build … -o /tmp/foo` writes to `/tmp/foo_<atom_name>.<ext>` for each atom (e.g. `/tmp/foo_main_fn.verified.json` for `--emit verified-json`). Don't assert on the literal `-o` path.
- **`--emit proof-cert`**: The `build --emit proof-cert` flag is parsed but returns empty artifacts. Use `verify --proof-cert` for actual cert generation.
- **`cmd_verify --proof-cert` Z3-unknown summary line is currently unreachable**: the new `ℹ  N atom(s) returned 'unknown' from Z3.` line in `src/main.rs::cmd_verify` (~lines 1058-1073) only fires when `cert.atoms[i].z3_check_result == "unknown"`. Every `cert_results.insert(...)` site in `src/main.rs` (lines 801, 845, 874, 913, 955, 983, 2413) inserts only `"unsat"` or `"sat"` — so the line is dead against current `.mm` sources. To exercise it adversarially, temporarily patch one `cert_results.insert(...)` call to record `"unknown"` (e.g. gated on a `MUMEI_TEST_INJECT_UNKNOWN=<atom_name>` env var), rebuild, run `mumei verify <file.mm> --proof-cert`, observe the line, then revert. The negative control (`--json`) must suppress the line.

## Running Tests
```bash
# Full workspace tests
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo test --workspace

# Lint
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo clippy --workspace
cargo fmt --check
```

## Devin Secrets Needed
None required — all testing is local.
