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
| `mumei verify --cross-spec-verify --report-dir <dir> <file.mm>` | Verify atoms and write `<dir>/cross_spec.json` for cross-specification consistency |
| `mumei verify --proof-cert <file.mm>` | Verify + generate `.proof.json` certificate. When run from repo root, the current CLI may write `./<stem>.proof.json` rather than beside the source file; trust the printed path. |
| `mumei verify --emit escalation-bundle --output <base> <file.mm>` | Verify + generate `<base>.escalation-bundle.json` for Lean escalation candidates |
| `mumei verify-cert <cert.json> <file.mm>` | Verify cert against current source |
| `mumei build <file.mm>` | Build (verify + compile to LLVM IR) |
| `mumei build --emit proof-cert <file.mm>` | Note: `--emit proof-cert` dispatches to `ProofCert` emit target but returns empty artifacts. Use `verify --proof-cert` instead for cert generation. |
| `mumei build --emit escalation-bundle --output <base> <file.mm>` | Build + generate `<base>.escalation-bundle.json`; escalatable verification failures should be deferred into the bundle rather than exiting before serialization |
| `mumei build --emit <unknown_name>` | Unknown emit targets fall through to `emitter::load_external_emitter`, which checks `~/.mumei/emitters/<name>/libmumei_emit_<name>.{so,dylib,dll}` (no `lib` prefix on Windows — matches Rust `cdylib` convention). When absent, exits 1 with stderr beginning `❌ Error: Unknown emit target …` and an external plugin lookup failure. |
| `mumei publish` | Publish current project to local registry (requires `mumei.toml`) |
| `mumei add <pkg>` | Add dependency from registry (requires `mumei.toml`) |
| `mumei verify --strict-imports <file.mm>` | Strict import mode |
| `mumei build --strict-imports <file.mm>` | Strict import mode for build |
| `mumei verify --allow-lean-verified <file.mm>` | Accept mumei-lean-emitted certificates (`z3_check_result == "lean_verified"`) as proven during import resolution. When this flag triggers acceptance, the resolver audit-logs `🔗 Lean-verified atom '<name>' accepted as proven (--allow-lean-verified)` to stderr — useful as a grep target for tests. |
| `mumei verify-cert --allow-lean-verified <cert.json> <file.mm>` | Accept `z3_check_result == "lean_verified"` atoms as proven during certificate verification; without the flag, those atoms should be reported as unproven |

## Test Files

- `sword_test.mm` — 8 atoms covering loops, floats, stack ops. Note: `scale` atom may fail verification (float multiplication precision), so `all_verified` can be `false`.
- `tests/test_cross_spec.mm` — Cross-specification fixture where `transfer` calls `validate_balance`; good for `--cross-spec-verify` report testing.
- `examples/import_test/main.mm` — Imports `lib/math_utils.mm`, good for import/dependency testing.
- `examples/import_test/lib/math_utils.mm` — 2 simple atoms (`safe_add`, `safe_double`), all verify successfully. Good for generating clean certificates and zero-candidate escalation bundles.
- `tests/test_contradiction.mm` — Existing contradiction fixture where `type Pos = i64 where v > 0` conflicts with `requires: n < 0`; useful for unsat-core and `report.json` diagnostics testing. For escalation-bundle testing, this should remain non-escalatable and should not emit candidates.
- `tests/test_cross_atom_chain.mm` — Effect system + chained atom composition.
- `tests/test_nested_while_no_trusted.mm` — Regression for `Expr::ArrayAccess => i64` MIR inference (`let key = arr[i]` inside nested `while`).
- `tests/test_verified_sort.mm` — Mirrors `std/list.mm::verified_insertion_sort` without `trusted`; same regression target as above plus the `forall(i, 0, n, arr[i] >= 0)` Z3-bounds idiom.
- `tests/test_polymorphic_array.mm` — Polymorphic `[T]` array verification fixture. Use this after parser/MIR/Z3 array changes and external emitter plugin-loader changes. It should verify 5 atoms: legacy untyped/i64 array access, `[f64]` reads, `[f64]` stores of an integer literal, `[bool]` reads/equality, and `[i64]` element inference.
- `tests/test_verified_ffi.mm` — Existing scalar `f64` FFI regression fixture. Useful when changes touch f64/Real/Float verification semantics.

## Testing Flows

### Cross-specification Verification Report

Use this flow when changes touch `mumei-core/src/cross_spec/`, `VerificationConfig`, module-level verification reporting, or CLI/manifest wiring for `cross_spec_verify`.

```bash
cd /home/ubuntu/repos/mumei
rm -rf /home/ubuntu/mumei-cross-spec-report
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei verify \
  --cross-spec-verify \
  --report-dir /home/ubuntu/mumei-cross-spec-report \
  tests/test_cross_spec.mm
```

Expected assertions:
- Command exits zero.
- Output includes `Cross-spec report written to: /home/ubuntu/mumei-cross-spec-report/cross_spec.json`.
- `/home/ubuntu/mumei-cross-spec-report/cross_spec.json` exists and is valid JSON.
- `summary.total_atoms == 6`, `summary.consistent_calls == 1`, `summary.inconsistent_calls == 0`, `summary.circular_dependency_count == 0`, and `summary.global_invariant_count == 2`.
- `contract_consistency` has exactly one edge: `transfer -> validate_balance` with `is_consistent == true`.
- `global_invariants` contains `result >= 0`.

A quick JSON assertion helper:
```bash
python - <<'PY'
import json
from pathlib import Path
p = Path('/home/ubuntu/mumei-cross-spec-report/cross_spec.json')
assert p.exists(), p
report = json.loads(p.read_text())
assert report['summary']['total_atoms'] == 6, report['summary']
assert report['summary']['consistent_calls'] == 1, report['summary']
assert report['summary']['inconsistent_calls'] == 0, report['summary']
assert report['summary']['circular_dependency_count'] == 0, report['summary']
assert report['summary']['global_invariant_count'] == 2, report['summary']
contracts = report['contract_consistency']
assert len(contracts) == 1, contracts
assert contracts[0]['caller_atom'] == 'transfer', contracts[0]
assert contracts[0]['callee_atom'] == 'validate_balance', contracts[0]
assert contracts[0]['is_consistent'] is True, contracts[0]
assert any(inv['invariant'] == 'result >= 0' for inv in report['global_invariants'])
PY
```

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

### Lean Escalation Bundle CLI and Bridge Dry-Run

Use this flow when changes touch `mumei-core/src/verification.rs`, `mumei-core/src/proof_cert.rs`, `src/main.rs` escalation CLI wiring, or the mumei-lean bridge schema.

```bash
cd /home/ubuntu/repos/mumei
rm -rf /home/ubuntu/mumei-escalation-e2e
mkdir -p /home/ubuntu/mumei-escalation-e2e
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei verify \
  --emit escalation-bundle \
  --output /home/ubuntu/mumei-escalation-e2e/verify-bundle \
  examples/import_test/lib/math_utils.mm
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei build \
  --emit escalation-bundle \
  --output /home/ubuntu/mumei-escalation-e2e/build-bundle \
  examples/import_test/lib/math_utils.mm
```

Expected assertions:
- Verify writes `/home/ubuntu/mumei-escalation-e2e/verify-bundle.escalation-bundle.json`.
- Build writes `/home/ubuntu/mumei-escalation-e2e/build-bundle.escalation-bundle.json`.
- Zero-candidate fixtures should have `summary.candidate_count == 0`, `summary.by_reason == {}`, and `candidates == []`.
- For `tests/test_contradiction.mm`, `build --emit escalation-bundle` should exit non-zero and should not write an escalation candidate bundle, because spec contradictions / `requires_unsat` are explicitly non-escalatable.

For cross-repo bridge compatibility, create a synthetic escalation bundle with a candidate containing `z3_check_result`, `z3_result_class`, `status`, `escalation_reason`, `logic_fragment_tags`, `requires`, `ensures`, hashes, dependency/effect fields, and optional `lean_metadata`, then run from `/home/ubuntu/repos/mumei-lean`:

```bash
python scripts/bridge.py \
  --escalation-bundle /home/ubuntu/mumei-escalation-e2e/synthetic-escalation-bundle.json \
  --out-dir /home/ubuntu/mumei-escalation-e2e/generated \
  --summary-json /home/ubuntu/mumei-escalation-e2e/summary.json \
  --module-prefix Generated \
  --no-build
```

Expected assertions:
- Command exits zero and generated Lean files exist under the output directory.
- `summary.json` includes `total_candidates`, `metrics.escalation_attempts`, `metrics.by_failure_reason.<reason>.attempts`, and `metrics.by_logic_fragment.<tag>.attempts`.
- `--no-build` is a dry-run mode and intentionally does not write `--lean-cert-out`; only expect Lean source and summary JSON from this step.

### Lean-Verified Certificate Opt-In

Use this flow when changes touch `verify-cert`, proof certificate verification, or `--allow-lean-verified` trust policy.

```bash
cd /home/ubuntu/repos/mumei
rm -f math_utils.proof.json cross_spec.json /home/ubuntu/mumei-escalation-e2e/lean-verified-cert.json
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei verify --proof-cert examples/import_test/lib/math_utils.mm
```

Transform the generated certificate at the path printed by the CLI (currently `./math_utils.proof.json` when run from the repo root) so each atom has `z3_check_result = "lean_verified"` and matching `lean_metadata.status = "lean_verified"`.

Expected assertions:
- `./target/debug/mumei verify-cert <lean-verified-cert> examples/import_test/lib/math_utils.mm` exits non-zero and reports the Lean-verified atoms as `unproven`.
- `./target/debug/mumei verify-cert --allow-lean-verified <lean-verified-cert> examples/import_test/lib/math_utils.mm` exits zero and reports those atoms as `proven`.
- Remove generated `math_utils.proof.json` and `cross_spec.json` afterward so the repo stays clean.

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
- `math_utils.proof.json` is created at the path printed by the CLI.
- Verify output includes 2 verified atoms or 2 skipped atoms if the verification cache is warm.
- `verify-cert` exits zero and prints a valid/verified certificate message.

## Notes

- Prefer using absolute `LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu` env vars for all cargo/CLI commands in this repo.
- No browser recording is useful for shell-only CLI flows; collect command output and generated JSON instead.
- Mumei verification commands may emit `cross_spec.json` in the current working directory; delete temporary copies before final `git status`.
