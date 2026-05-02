---
name: testing-mumei-cli
description: Test Mumei CLI verification, build, proof certificate, and polymorphic array flows locally. Use when validating Mumei language/runtime/compiler changes through the CLI.
---
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
| `mumei build --emit <unknown_name> <file.mm>` | Unknown emit targets fall through to `emitter::load_external_emitter`, which checks `~/.mumei/emitters/<name>/libmumei_emit_<name>.{so,dylib,dll}` (no `lib` prefix on Windows — matches Rust `cdylib` convention). When absent, exits 1 with stderr beginning `❌ Error: Unknown emit target …` and an external plugin lookup failure. |
| `mumei publish` | Publish current project to local registry (requires `mumei.toml`) |
| `mumei add <pkg>` | Add dependency from registry (requires `mumei.toml`) |
| `mumei verify --strict-imports <file.mm>` | Strict import mode |
| `mumei build --strict-imports <file.mm>` | Strict import mode for build |
| `mumei verify --allow-lean-verified <file.mm>` | Accept mumei-lean-emitted certs (`z3_check_result == "lean_verified"`) as proven. When this flag triggers acceptance, the resolver audit-logs `🔗 Lean-verified atom '<name>' accepted as proven (--allow-lean-verified)` to stderr — useful as a grep target for tests. |

## Test Files

- `sword_test.mm` — 8 atoms covering loops, floats, stack ops. Note: `scale` atom may fail verification (float multiplication precision), so `all_verified` can be `false`.
- `examples/import_test/main.mm` — Imports `lib/math_utils.mm`, good for import/dependency testing.
- `examples/import_test/lib/math_utils.mm` — 2 simple atoms (`safe_add`, `safe_double`), all verify successfully. Good for generating clean certificates.
- `tests/test_cross_atom_chain.mm` — Effect system + chained atom composition.
- `tests/test_nested_while_no_trusted.mm` — Regression for `Expr::ArrayAccess => i64` MIR inference (`let key = arr[i]` inside nested `while`).
- `tests/test_verified_sort.mm` — Mirrors `std/list.mm::verified_insertion_sort` without `trusted`; same regression target as above plus the `forall(i, 0, n, arr[i] >= 0)` Z3-bounds idiom.
- `tests/test_polymorphic_array.mm` — Polymorphic `[T]` array verification fixture. Use this after parser/MIR/Z3 array changes and external emitter plugin-loader changes. It should verify 5 atoms: legacy untyped/i64 array access, `[f64]` reads, `[f64]` stores of an integer literal, `[bool]` reads/equality, and `[i64]` element inference.
- `tests/test_verified_ffi.mm` — Existing scalar `f64` FFI regression fixture. Useful when changes touch f64/Real/Float verification semantics.

## Testing Flows

### Polymorphic `[T]` Array Verification

Use this flow when changes affect array type parsing, MIR array element inference, Z3 array sort selection, or ArrayStore coercion.

```bash
cd /home/ubuntu/repos/mumei
rm -rf tests/.mumei tests/.mumei_build_cache tests/.mumei_cache .mumei .mumei_build_cache .mumei_cache
./target/debug/mumei verify tests/test_polymorphic_array.mm
```

Expected assertions:
- Output includes `⚖  'test_i64_array': verified`.
- Output includes `⚖  'test_f64_array': verified`.
- Output includes `⚖  'test_f64_array_store_int_literal': verified`.
- Output includes `⚖  'test_bool_array': verified`.
- Output includes `⚖  'test_array_element_type_inference': verified`.
- Final summary includes `Verification passed: 5 item(s) verified`.
- Output does not include `skipped`, `failed`, a Z3 sort/store mismatch, or `Array store value must be real`.

The `test_f64_array_store_int_literal` atom is the adversarial case for storing an integer literal such as `42` into a `[f64]` array. If Int-to-Real coercion before Z3 `array.store` is broken, this case may fail with a sort mismatch.

### External Emitter Plugin Loader

Use this flow when changes touch `mumei-core/src/emitter.rs`, external `--emit <name>` dispatch, plugin ABI (`EmitterPluginHandle`), or plugin library lifetime/reload behavior.

1. Build a temporary Rust `cdylib` plugin whose crate name is `mumei_emit_counting`, with:
   - `mumei_emitter_abi_version() -> EMITTER_ABI_VERSION`
   - `mumei_create_emitter() -> EmitterPluginHandle`
   - a factory side effect that appends one line to `/home/ubuntu/mumei-plugin-test/factory.log`
   - an `emit()` implementation that returns one `ArtifactKind::Metadata` marker named `format!("{}.counting.marker", output_path.display())` containing `counting:<atom_name>`
2. Install the compiled Linux artifact at the exact loader path:
   ```bash
   mkdir -p /home/ubuntu/.mumei/emitters/counting
   cp /home/ubuntu/mumei-plugin-test/plugin/target/release/libmumei_emit_counting.so \
     /home/ubuntu/.mumei/emitters/counting/libmumei_emit_counting.so
   ```
3. Run the five-atom fixture:
   ```bash
   LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 /home/ubuntu/repos/mumei/target/debug/mumei \
     build /home/ubuntu/repos/mumei/tests/test_polymorphic_array.mm \
     --emit counting \
     -o /home/ubuntu/mumei-plugin-test/out/result
   ```
4. Expected assertions:
   - command exits 0
   - output includes `Blade forged successfully with 5 atoms`
   - output includes `Compiled '<atom>' to external plugin 'counting'.` for all five atoms in `tests/test_polymorphic_array.mm`
   - marker files exist at `/home/ubuntu/mumei-plugin-test/out/result_<atom>.counting.marker`
   - every marker contains exact text `counting:<atom>`
   - `wc -l /home/ubuntu/mumei-plugin-test/factory.log` is exactly `1`
   - combined output does not contain `Unknown emit target`, `plugin loader is not yet implemented`, `ABI version mismatch`, or `does not export mumei_create_emitter`

The factory count is the adversarial assertion: a broken eager CLI probe plus per-atom reload may produce 6 calls for this fixture; per-atom reload alone may produce 5; the fixed behavior should produce exactly 1.

For the negative control, run the same `build` with `--emit definitely_missing_phase3_plugin`. It should exit non-zero and stderr should include the plugin name plus the `.mumei/emitters` expected install path.

### Scalar `f64` Regression Verification

Use this flow when changes touch f64 verification semantics, especially Float-vs-Real handling.

```bash
cd /home/ubuntu/repos/mumei
rm -rf tests/.mumei tests/.mumei_build_cache tests/.mumei_cache .mumei .mumei_build_cache .mumei_cache
./target/debug/mumei verify tests/test_verified_ffi.mm
```

Expected final summary: `Verification passed: 3 item(s) verified`.

### Certificate Generation + Verification (P5-A)
1. `mumei verify --proof-cert <file.mm>` generates `<stem>.proof.json`.
2. `mumei verify-cert <cert.json> <file.mm>` checks cert against source.
3. Modify source, re-run verify-cert to see "changed" status.

### Publish + Add (P5-B)
1. Create project dir with `mumei.toml` (needs `[package]` with name/version and `[build]` with entry).
2. `mumei publish` from project dir.
3. Check `~/.mumei/registry.json` for `cert_path`/`cert_hash`.
4. Create consumer project, `mumei add <pkg-name>` to resolve from registry.

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
- **Verification caching — TWO locations**: To force a clean re-verify you need to clear both `.mumei/cache/verification_cache.json` (enhanced cache) and `<input-dir>/.mumei_cache` (legacy resolver cache). After only clearing one, verify can report `0 verified, N skipped (unchanged) ⚡`. Quick reset:
  ```bash
  rm -rf .mumei .mumei_build_cache <input-dir>/.mumei_cache
  ```
- **`-o` is a base name, not a full path**: `mumei build … -o /tmp/foo` writes to `/tmp/foo_<atom_name>.<ext>` for each atom (e.g. `/tmp/foo_main_fn.verified.json` for `--emit verified-json`). Do not assert on the literal `-o` path.
- **`--emit proof-cert`**: The `build --emit proof-cert` flag is parsed but returns empty artifacts. Use `verify --proof-cert` for actual cert generation.
- **`cmd_verify --proof-cert` Z3-unknown summary line might be unreachable**: the `ℹ  N atom(s) returned 'unknown' from Z3.` line in `src/main.rs::cmd_verify` only fires when `cert.atoms[i].z3_check_result == "unknown"`. Most current `.mm` sources record only `"unsat"` or `"sat"`. To exercise it adversarially, temporarily patch one `cert_results.insert(...)` call to record `"unknown"` (for example gated on a test env var), rebuild, run `mumei verify <file.mm> --proof-cert`, observe the line, then revert. The negative control (`--json`) must suppress the line.

## Running Tests
```bash
# Full workspace tests
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo test --workspace

# Lint
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Devin Secrets Needed
None required — all testing is local.
