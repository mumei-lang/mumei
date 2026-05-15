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

## Key CLI Commands for Testing

| Command | Purpose |
|---|---|
| `mumei verify <file.mm>` | Verify atoms with Z3 |
| `mumei verify --report-dir <dir> <file.mm>` | Verify and write `<dir>/report.json` for diagnostics/self-healing flows |
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
- `tests/test_contradiction.mm` — Existing contradiction fixture where `type Pos = i64 where v > 0` conflicts with `requires: n < 0`; useful for unsat-core and `report.json` diagnostics testing.
- `tests/test_cross_atom_chain.mm` — Effect system + chained atom composition.
- `tests/test_nested_while_no_trusted.mm` — Regression for `Expr::ArrayAccess => i64` MIR inference (`let key = arr[i]` inside nested `while`).
- `tests/test_verified_sort.mm` — Mirrors `std/list.mm::verified_insertion_sort` without `trusted`; same regression target as above plus the `forall(i, 0, n, arr[i] >= 0)` Z3-bounds idiom.
- `tests/test_polymorphic_array.mm` — Polymorphic `[T]` array verification fixture. Use this after parser/MIR/Z3 array changes and external emitter plugin-loader changes. It should verify 5 atoms: legacy untyped/i64 array access, `[f64]` reads, `[f64]` stores of an integer literal, `[bool]` reads/equality, and `[i64]` element inference.
- `tests/test_verified_ffi.mm` — Existing scalar `f64` FFI regression fixture. Useful when changes touch f64/Real/Float verification semantics.

## Testing Flows

### Contradiction Report / Unsat Core Diagnostics

Use this flow when changes touch contradiction handling, Z3 unsat-core tracking labels, semantic feedback, `report.json`, or self-healing diagnostics.

```bash
cd /home/ubuntu/repos/mumei
rm -rf /home/ubuntu/mumei-contradiction-report
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei verify \
  --report-dir /home/ubuntu/mumei-contradiction-report \
  tests/test_contradiction.mm
```

Expected assertions:
- Command exits non-zero because `tests/test_contradiction.mm` is intentionally contradictory.
- Output includes `Contradiction found`.
- `/home/ubuntu/mumei-contradiction-report/report.json` exists.
- Report top-level `failure_type` is `invariant_violated`.
- `semantic_feedback.raw_unsat_core` includes `track_refined_type_n::Pos` and `track_requires`.
- If testing minimal-core support, `semantic_feedback.minimal_unsat_core` should include exactly `track_refined_type_n::Pos` and `track_requires`, with `minimal_core_size == 2`, `total_core_size == 2`, and `reduction_ratio == 1.0`.
- If testing suggestion text, check `semantic_feedback.suggestion`; the top-level `suggestion` may still be the broader contextual invariant suggestion.

A quick JSON assertion helper:

```bash
python - <<'PY'
import json
from pathlib import Path
report = json.loads(Path('/home/ubuntu/mumei-contradiction-report/report.json').read_text())
sf = report['semantic_feedback']
expected = {'track_refined_type_n::Pos', 'track_requires'}
assert report['failure_type'] == 'invariant_violated'
assert set(sf['raw_unsat_core']) == expected
if 'minimal_unsat_core' in sf:
    assert set(sf['minimal_unsat_core']) == expected
    assert sf['minimal_core_size'] == 2
    assert sf['total_core_size'] == 2
    assert sf['reduction_ratio'] == 1.0
PY
```

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

```bash
cd /home/ubuntu/repos/mumei
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei build --emit missing-plugin examples/import_test/lib/math_utils.mm
```

Expected assertions:
- Command exits non-zero.
- Stderr begins with `❌ Error: Unknown emit target 'missing-plugin'`.
- Stderr includes the checked plugin path under `~/.mumei/emitters/missing-plugin/`.
- Stderr does not panic or print `segmentation fault`.

### Proof Certificate Generation and Verification

Use this flow when changes touch proof certificate generation, certificate verification, hashing, import caching, or resolver trust policy.

```bash
cd /home/ubuntu/repos/mumei
rm -f examples/import_test/lib/math_utils.proof.json
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei verify --proof-cert examples/import_test/lib/math_utils.mm
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei verify-cert examples/import_test/lib/math_utils.proof.json examples/import_test/lib/math_utils.mm
```

Expected assertions:
- `math_utils.proof.json` is created.
- Verify output includes 2 verified atoms.
- `verify-cert` exits zero and prints a valid/verified certificate message.

## Notes

- Prefer using absolute `LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu` env vars for all cargo/CLI commands in this repo.
- No browser recording is useful for shell-only CLI flows; collect command output and generated JSON instead.
