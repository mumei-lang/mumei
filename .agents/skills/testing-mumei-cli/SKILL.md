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
| `mumei verify --disable-spurious-detection <file.mm>` | Disable P8-A spurious counterexample classification and use the legacy SAT/postcondition failure path |
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

### P8-A Spurious Counterexample Detection

Use this flow when changes touch `mumei-core/src/verification/spurious_detection.rs`, Z3 SAT/counterexample handling in `executor.rs`, `VerificationConfig::enable_spurious_detection`, proof certificate metadata, or the `--disable-spurious-detection` CLI flag.

First run the focused Rust tests:
```bash
cd /home/ubuntu/repos/mumei
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo test --test test_spurious_detection
```
Expected assertions:
- The four P8-A regression tests pass: validated counterexample, uninterpreted-symbol spurious candidate, unused hypothesis detection, and minimal constraint set.

Create a temporary validated-counterexample fixture outside the repo:
```bash
mkdir -p /home/ubuntu/mumei-p8a-fixtures
cat > /home/ubuntu/mumei-p8a-fixtures/validated_counterexample.mm <<'MM'
atom bad_increment(x: i64) -> i64 {
    requires: x >= 0;
    ensures: result > 10;
    body: {
        x + 1
    }
}
MM
```
Run:
```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei verify /home/ubuntu/mumei-p8a-fixtures/validated_counterexample.mm
```
Expected assertions:
- Command exits non-zero because the postcondition is intentionally false.
- Output includes `Counterexample validated for atom 'bad_increment'`.
- Output does not include `Spurious counterexample candidate`.

Create a temporary trusted-atom fixture to exercise trusted/uninterpreted provenance through the CLI:
```bash
cat > /home/ubuntu/mumei-p8a-fixtures/spurious_trusted.mm <<'MM'
extern "Rust" {
    fn native_external(x: i64) -> i64;
}

trusted atom external_value(x: i64) -> i64 {
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        native_external(x)
    }
}

atom depends_on_trusted(x: i64) -> i64 {
    requires: x >= 0;
    ensures: external_value(x) == 42;
    body: {
        x
    }
}
MM
```
Run:
```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei verify /home/ubuntu/mumei-p8a-fixtures/spurious_trusted.mm
```
Expected assertions:
- Command exits non-zero.
- Output includes `Spurious counterexample detected for atom 'depends_on_trusted'`.
- Output includes `Spurious counterexample candidate for atom 'depends_on_trusted'`.
- Output names `external_value (trusted_atom)` as the dependency/provenance.

Then verify the disable flag preserves the legacy failure path:
```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei verify --disable-spurious-detection /home/ubuntu/mumei-p8a-fixtures/spurious_trusted.mm
```
Expected assertions:
- Command exits non-zero.
- Output includes a normal postcondition/verification failure.
- Output does not include `Spurious counterexample candidate` or `Spurious counterexample detected`.

For certificate metadata, run the Proof Certificate flow below and additionally assert:
- Each atom has `symbol_provenance` as an array.
- `counterexample_validation` and `unused_hypotheses`, when present, are objects or `null`.
- Clean up generated `cross_spec.json`, proof cert files, and `/home/ubuntu/mumei-p8a-fixtures` after the test.

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

### Session-Type Protocol Checks (multi-file, `--cross-spec-files`)

Use this flow when changes touch `mumei-core/src/cross_spec/session_types.rs`,
`CrossSpecVerifier::verify_all()`, or `src/pipeline.rs` (`load_cross_spec_files`,
`annotate_atom_source_file`).

Argument order is non-obvious and easy to get wrong:

```bash
cd /home/ubuntu/repos/mumei
rm -rf /tmp/sess && mkdir -p /tmp/sess
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei verify \
  --report-dir /tmp/sess \
  --cross-spec-files tests/fixtures/session_types/order_server.mm \
  tests/fixtures/session_types/order_client.mm
```

- Extra `.mm` files go **after** `--cross-spec-files`, **comma-separated only**
  (`--cross-spec-files a.mm,b.mm`, or one `--cross-spec-files` per file). Space
  separation does not work: the second path is parsed as a positional argument
  and `mumei verify` fails with `error: unexpected argument`.
- The **primary** file must come **last** as the positional argument.
- `--report-dir` is what makes `cross_spec.json` appear. Without it there is no
  machine-checkable output.
- Results live in `cross_spec.json` under `session_protocol_violations` and
  `summary.session_protocol_violation_count`. Each violation makes
  `mumei verify` exit nonzero; in `mumei build` cross-spec verification it exits 1.

Violation kinds and the conditions that trigger them (useful for writing fixtures):

| kind | trigger |
| --- | --- |
| `duality_mismatch` | a send's post-state has no receiving atom **in a different file** (a continuation in the sender's own file does NOT satisfy duality) |
| `unreachable_receive` | a role's pre-state is not reachable by BFS from the effect's `initial` state (this catches disconnected role "islands" that only feed each other) |
| `deadlock_no_progress` | no reachable state is quiescent |

Gotchas that silently produce "no violations" (verify these before calling a
detector broken, and before calling a clean run a real pass):
- The effect is skipped entirely when it has more than `MAX_PROTOCOL_NODES` (32)
  states, more than `MAX_PROTOCOL_ROLES` (64) roles, fewer than 2 roles, or roles
  spanning fewer than 2 files. A >32-state protocol gets **no** session checking
  at all — expected-by-design, but it means genuine violations in large protocols
  are suppressed. Limit-based skips are visible: check
  `session_analysis_skips[]` / `summary.session_analysis_skipped_count` in
  `cross_spec.json` and the `Warning: session protocol not checked: ...` CLI
  line (role/file-count skips are not reported).
- Roles whose atom has no source-file attribution are skipped.
- Single-file protocols are intentionally never reported here; temporal behavior
  inside one file is the Temporal Effect Verifier's job.
- **Clear caches before every run** (`rm -rf .mumei .mumei_cache .mumei_build_cache`
  beside every source file involved) or you get `skipped (unchanged, cached)` and a
  false pass.

To prove a reachability/duality change actually altered behavior, diff the same
fixtures across two binaries using a git worktree of the base commit:

```bash
git worktree add -f /tmp/base <base-commit>
(cd /tmp/base && LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 \
   LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build)   # ~1 min incremental
# run identical commands with /tmp/base/target/debug/mumei vs ./target/debug/mumei
git worktree remove --force /tmp/base                      # keeps repo clean
```

### Proof-Aware Runtime Monitors (`--emit runtime-monitor`)

Use this flow when changes touch `mumei-emit-monitor/`,
`mumei-core/src/trust_boundary.rs`, or emitter dispatch in `src/codegen.rs`.

```bash
mkdir -p /tmp/mon
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei build tests/fixtures/runtime_monitor/trusted_boundary.mm \
  --emit runtime-monitor --output /tmp/mon/out
```

- Files are written as `<output-base>_<atom>.monitor.rs`. Monitors are emitted
  **only** for trust-boundary atoms; a proven pure atom must produce **zero**
  files (zero-cost). The `boundary:` tag is a `+`-joined combination, e.g.
  `trusted_atom`, `trusted_atom+extern_ffi`, `effect_pre_override`.
- The three boundary kinds to cover with fixtures: a `trusted atom`, an atom
  backed by an `extern "Rust" { ... }` declaration of the same name, and an atom
  with a non-empty `effect_pre`.
- Strongest selectivity test: put a `trusted atom` and a pure atom in **one**
  file and assert exactly one monitor is produced.

Compiling a generated monitor standalone — note the filename contains dots, so
rustc cannot infer a crate name and you must pass `--crate-name`:

```bash
rustc --edition 2021 --crate-type lib -D warnings \
  --crate-name mon_check -o /tmp/mon.rlib /tmp/mon/out_read_sensor.monitor.rs
```

Generated code must contain no `panic!`/`assert!` and must read `OTEL_ENABLED`
(NoOp when unset/false) and `OTEL_EXPORTER_OTLP_ENDPOINT`
(default `http://localhost:4318`).

To exercise a monitor at **runtime** (much stronger than grepping the source),
`include!` the unmodified generated file inside its own module in a small driver
and link a stub for the wrapped atom. The module wrapper is required: the
generated `extern "C" { fn <atom>(..) }` declaration otherwise collides with your
stub definition at name resolution instead of resolving at link time.

```rust
mod monitor { include!("/tmp/mon/out_mon_send.monitor.rs"); }
use monitor::{mon_send_monitored, mumei_monitor};
#[no_mangle] pub extern "C" fn mon_send(x: i64) -> i64 { x }
fn main() {
    mumei_monitor::set_effect_state_probe(|e| (e == "MonChannel").then(|| "Sent".to_string())).ok();
    mon_send_monitored(-5); // violations go to the hook, or stderr by default
}
```

Then assert: with `OTEL_ENABLED` unset stderr is empty; with `OTEL_ENABLED=true`
violations print as `mumei.monitor.contract_violation atom=... contract=... observed=...`;
`effect_pre` violations are only reported when an effect-state probe is installed
(without a probe the state is unobservable and nothing is reported).

Zero-cost dependency gate — must print nothing:
```bash
cargo tree --edges no-dev | grep -i opentelemetry
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

### Multi-return Tuple Result Indexing

Use this flow when changes touch tuple return types (`-> (T0, T1, ...)`), the
`Expr::ArrayAccess` arm in `mumei-core/src/verification/translator/expr.rs`,
tuple component seeding in `z3_types.rs` / `spec_validation.rs` / `executor.rs`,
or `tuple_component_types` / `is_unsupported_clause_error`.

The feature lets contracts on tuple-returning atoms index the result
(`result[0]`, `result[1]`) with each component encoded as its own typed Z3
value (heterogeneous types supported, e.g. `u64`→Int, `bool`→Bool).

**IMPORTANT — clear the verification cache before every run.** A warm cache
prints `'X': skipped (unchanged, cached)` and hides the real verdict, producing
a false pass. Use a fresh temp dir per run, or `rm -rf .mumei .mumei_cache
.mumei_build_cache` beside the source first.

Best signal is a BEFORE/AFTER comparison (build one binary from `develop`, one
from the branch). Fixture (`SafeAdd`, the #407 motivating example):
```
trusted atom SafeAdd(x: u64, y: u64) -> (u64, bool)
requires: x + y <= 2**64 - 1;
ensures: result[0] == x + y && result[1] == false;
body: x + y;
```
Expected assertions:
- BEFORE (feature absent): exit 1, output contains
  `failed to lower ensures clause 'result[1] == false'` / `Expected bool for ==`
  (heterogeneous bool component is un-encodable → `spec_lowering_failed`).
- AFTER: exit 0, output contains `'SafeAdd': verified` and `1 item(s) verified`,
  and does NOT contain `Skipped unsupported Z3 clause`, `satisfiable_with_skips`,
  `unverifiable`, or `Expected bool`.

Adversarial check that components are genuinely constrained (not vacuous). Use
`ensures: result[0] == x + y && result[0] == x + y + 1;`:
- AFTER: exit 1, output contains `ensures clauses are mutually inconsistent` /
  `ensures_conflict`. If tuple indices were left unconstrained this would falsely
  report `verified`.

Note: `body:` tuple construction and call-result indexing (`foo(...)[0]`) are out
of scope; `SafeAdd` must be a `trusted atom` (contract-only, body not checked).

### OpenTelemetry / TRACEPARENT Distributed Tracing

Use this flow when changes touch `src/telemetry.rs`, `Cargo.toml` `otel` feature, `#[cfg(feature = "otel")]` span instrumentation in `verify.rs` or `executor.rs`, or the Python-side `current_traceparent()` / `_env_with_traceparent()` in `mumei-agent`.

**Zero-cost check** — confirm `opentelemetry` is absent from the default dep tree:
```bash
cargo tree --edges no-dev | grep -i opentelemetry
# Expected: no output (empty)
```

**Both build targets compile**:
```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build --features otel
```

**Output equivalence** — clear the verification cache before each run to avoid false diffs from cache hits:
```bash
rm -rf .mumei_cache .mumei_build_cache .mumei examples/import_test/lib/.mumei_cache examples/import_test/lib/.mumei_build_cache examples/import_test/lib/.mumei
./target/debug/mumei verify examples/import_test/lib/math_utils.mm > /tmp/out-default.txt 2>/dev/null
# rebuild with --features otel, clear cache again, run again
diff /tmp/out-default.txt /tmp/out-otel.txt
# Expected: no diff
```

**OTEL_ENABLED=true graceful degradation** (no collector running):
```bash
OTEL_ENABLED=true ./target/debug/mumei verify examples/import_test/lib/math_utils.mm
# Expected: exit 0, normal verification output, no crash
```

**Valid TRACEPARENT accepted**:
```bash
OTEL_ENABLED=true TRACEPARENT="00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01" \
  ./target/debug/mumei verify examples/import_test/lib/math_utils.mm
# Expected: exit 0, output unchanged vs without TRACEPARENT
```

**Invalid TRACEPARENT ignored**:
```bash
OTEL_ENABLED=true TRACEPARENT="invalid-garbage-value" \
  ./target/debug/mumei verify examples/import_test/lib/math_utils.mm
# Expected: exit 0, no crash, no panic, verification succeeds normally
```

**Python-side no-op** (in mumei-agent repo):
```bash
OTEL_ENABLED=false uv run python3 -c "from agent import telemetry; assert telemetry.current_traceparent() is None"
OTEL_ENABLED=false uv run python3 -c "from agent.mumei_client import MumeiClient; assert MumeiClient._env_with_traceparent() is None"
```

### Exponent (`**`) and Chained-Comparison Lowering

Use this flow when changes touch `**`/`Op::Pow` handling, `normalize_comparison_chains`
in `parser/expr.rs`, `parse_expression` in `parser/mod.rs`, or Z3 clause lowering in
`verification/translator/expr.rs` / `spec_validation.rs`.

Best evidence is a **BEFORE/AFTER** comparison built via a git worktree:
```bash
cd /home/ubuntu/repos/mumei
git worktree add -f /home/ubuntu/mumei-develop-base origin/develop   # or the PR base
( cd /home/ubuntu/mumei-develop-base && LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 \
  LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build )
# ...run both target/debug/mumei binaries on the same fixture, clearing cache each time...
git worktree remove --force /home/ubuntu/mumei-develop-base   # cleanup
```

Fixture (`tests/test_pow_chain.mm`) exercising the geth overflow idiom:
```
atom safe_add_with_bound(x: i64, y: i64) -> i64
requires: x >= 0 && y >= 0 && x + y <= 2**64 - 1;
ensures: 0 <= result <= 2**64 - 1 && result == x + y;
body: x + y;
```
Expected assertions:
- AFTER: `⚖  'safe_add_with_bound': verified ✅`, exit 0. BEFORE (no `**`/chain support):
  hard-fails with `spec_lowering_failed` / `Expected int` on `0 <= result <= 2 * * 64 - 1`.
- Full-precision fold is adversarial: `ensures: 2**64 - 1 > 9223372036854775807;` (RHS is
  i64::MAX) must **verify**, and the flipped `<=` must **fail** (`ensures_unsat`). A verify
  on the `<=` form would mean the fold truncated/overflowed i64 — the fold must use decimal
  string math (`Int::from_str`), not i64.
- Chained comparison `a <= b <= c` should lower to a conjunction; a `parse_expression`
  unit test asserting this guards against regressions in un-normalized paths (vacuity,
  spurious-detection, call-graph, refinement checks all route through `parse_expression`).
- A symbolic/non-constant exponent (`x**y`) in an ensures clause is now surfaced as a clean
  `unverifiable ⚠` verdict end-to-end (see the flow below); older builds instead hard-errored
  with `Unsupported exponentiation` (exit 1). Both are sound; the verdict form is the current
  behavior.

### Executor `unverifiable` Verdict for Unencodable Clauses

Use this flow when changes touch the executor proof path in `verification/executor.rs`
(esp. `lower_clause_with_skip` / `ClauseLoweringOutcome`, the requires/ensures loops), the
tri-state verdict plumbing in `commands/verify.rs`, or `split_top_level_conjunctions` /
`strip_wrapping_parens` in `spec_validation.rs`. Clear caches (`rm -rf .mumei tests/.mumei
cross_spec.json`) before every run — a warm cache masks verdict changes.

Fixtures and expected verdicts (each distinguishes a broken build):
- Symbolic exponent in ensures (`ensures: result == x**y && result == x;`) → `unverifiable ⚠`,
  exit 1. Must NOT be `verified` and must NOT hard-error.
- Encodable-but-false postcondition (`ensures: result > x + 1; body: x;`) → `refuted/failed`,
  `0 unverifiable`, exit 1. Guards that a genuine counterexample is never downgraded to
  unverifiable.
- Trivially-true conjunct (`ensures: result == x && true; body: x;`) → `verified`, exit 0.
  A broken build reports `unverifiable` (trivial `true`/empty conjunct misclassified as an
  unsupported skip — `split_top_level_conjunctions` yields `["result == x", "true"]`, and
  `lower_clause_with_skip` must return `Trivial`, not `Skipped`, for the `true` conjunct).
- `(A && B) || (C && D)` ensures (e.g. `std/option.mm::option_unwrap_or`) → `verified`, no
  `Spurious counterexample`. A broken `strip_wrapping_parens` mangles the disjunction.
- Mixed multi-atom file (unverifiable atom then refuted atom) → summary
  `0 passed, 1 failed, 1 unverifiable`; the refuted atom must stay `failed`, not read a stale
  shared `report.json` status. Verdict detection is error-based, not stale-file-based.

Machine-readable surfacing:
- `--report-dir <dir>`: `report.json` has `status="unverifiable"`,
  `reason="Skipped unsupported Z3 clause(s) in ensures."`, and a per-clause `diagnostics`
  entry (`"Skipped unsupported Z3 clause: ensures clause '...': ... Unsupported exponentiation ..."`).
  This is the artifact mumei-agent consumes; assert against it.
- `--json` stdout: exposes `status="unverifiable"` + `reason`, but its `diagnostics` array may
  be `[]` — `enrich_verify_json_payload` (`src/feedback.rs`) overwrites report diagnostics with
  the CLI-level vector. This is pre-existing and also affects the #408 spec-skip path; prefer
  `--report-dir`/`report.json` for diagnostic assertions rather than `--json` stdout.

## Notes

- Prefer using absolute `LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu` env vars for all cargo/CLI commands in this repo.
- No browser recording is useful for shell-only CLI flows; collect command output and generated JSON instead.
- Mumei verification commands may emit `cross_spec.json` in the current working directory; delete temporary copies before final `git status`.
- When comparing verify output between two binaries, always clear `.mumei_cache` / `.mumei_build_cache` / `.mumei` directories before each run — the verification cache causes "skipped (unchanged, cached)" lines that create false diffs.

## Session-type protocol checks (`--cross-spec-files`)

The flows in this section and the two that follow need the P22/P23 Session
Types and Proof-Aware Observability work (mumei-lang/mumei#499); on a branch
without it there is no `session_protocol_violations`, no
`session_analysis_skips`, and no `runtime-monitor` emit target.

Cross-file session protocol violations (`duality_mismatch`,
`unreachable_receive`, `deadlock_no_progress`) are produced by `mumei verify`
when the peer files are passed explicitly. `verify` always writes
`cross_spec.json` — into the current working directory by default, or into
`--report-dir` when given, which is what keeps runs from clobbering each other:

```bash
rm -rf .mumei .mumei_cache .mumei_build_cache   # warm caches print "skipped (unchanged, cached)"
mumei verify --report-dir /tmp/rd \
  --cross-spec-files protocol.mm,server.mm client.mm
```

`--cross-spec-files` accepts **comma-separated** paths or repeated flags only.
Space-separated extra paths become positionals and fail with
`error: unexpected argument`. The primary file goes last.

Each violation counts as a failure, so `verify` exits nonzero. Three conditions
silently yield "no violations" and are easy to mistake for a pass: states > 32,
fewer than 2 roles, or fewer than 2 distinct source files. Single-file protocols
are intentionally not reported (that is the Temporal Effect Verifier's job), and
so is a protocol whose peer roles are different atoms in the same file — that is
what a verified library owning both ends looks like (`std/ownership.mm`).

## Bounded session analysis: skips are reported, not silent

When an effect exceeds a bound the analysis skips it and says so, rather than
failing open silently. `cross_spec.json` gains `session_analysis_skips[]`
(`effect`, `reason`, `state_count`, `role_count`, `limit`, `message`) plus
`summary.session_analysis_skipped_count`, and the CLI prints
`Warning: session protocol not checked: ...` once per skipped effect.

- `state_limit_exceeded` — `states > MAX_PROTOCOL_NODES` (32); `role_count` is
  `null` because roles are never collected.
- `role_limit_exceeded` — `roles > MAX_PROTOCOL_ROLES` (64); `role_count` is
  populated. To build one, keep states small (e.g. 3) and generate >64 atoms
  declaring `effect_pre`/`effect_post` for the same effect across two files.

Skips are warnings, never failures: exit stays 0. Genuine violations remain hard
errors. Skips are deduplicated by **unqualified** effect name, so an effect
visible both as `Channel` and `protocol::Channel` warns once — but two
differently-named effects each warn, so don't assume "one warning" in general.
Two files that both declare `effect Channel` also collapse to one entry in the
module environment (`effect_defs` is keyed by name), upstream of this dedup.

## Session violations through `mumei build`

`mumei build` has **no** `--report-dir`, `--cross-spec-files`, or
`--cross-spec-verify` flags — cross-spec verification is manifest-driven
(`ProofConfig::cross_spec_verify`, default on) and it sees peer files only
through `import`. `cross_spec.json` lands in the resolved output directory (the
parent of `--output`), falling back to the working directory, so a build run
with `--output /tmp/out/out` leaves it at `/tmp/out/cross_spec.json` — don't
search the repo root for it.

Role collection drops any atom without `source_file` attribution
(`session_types.rs::collect_roles`). `pipeline.rs::annotate_source_file` covers
only the primary input, so imported atoms rely on
`resolver/imports.rs` attributing them to the resolved import path; a fixture
whose entry file merely `import`s both roles is therefore the way to exercise
build-path enforcement, and it must exit 1 with
`❌ Session protocol violation (...)` on stderr. If a build-path run comes back
"clean", cross-check the same files through `verify --cross-spec-files` before
believing it — a silently unattributed role looks exactly like a pass.

## Proof-aware runtime monitors (`--emit runtime-monitor`)

```bash
mumei build fixture.mm --emit runtime-monitor --output /tmp/out/out
```

Monitors are emitted only for trust-boundary atoms (`trusted` atoms, extern/FFI
atoms, atoms with `effect_pre`) as `<base>_<atom>.monitor.rs`; proven pure atoms
must produce no file at all. Note that "not every atom emits a monitor" is a bad
assertion for files like `std/ownership.mm` where every atom carries
`effect_pre` — assert the exact expected set instead.

To compile a generated monitor standalone you must pass `--crate-name`, because
the `<base>_<atom>.monitor.rs` filename contains dots and rustc cannot infer a
crate name from it:

```bash
rustc --edition 2021 --crate-type lib -D warnings \
  --crate-name mon_probe -o /tmp/mon.rlib /tmp/out/out_read_sensor.monitor.rs
```

To exercise a monitor at runtime, `include!` it inside a `mod` wrapper and
provide the extern symbol it wraps, otherwise the stub collides with the
generated function. Behaviour depends on env vars: with `OTEL_ENABLED` unset the
monitor is a true no-op (zero bytes on stderr); with `OTEL_ENABLED=true`
violations are reported via hook/stderr and it never panics.
`OTEL_EXPORTER_OTLP_ENDPOINT` defaults to `http://localhost:4318`. `effect_pre`
violations are only observable after installing
`mumei_monitor::set_effect_state_probe(...)`; without a probe the state is
unobservable and nothing is reported.

## Capability declarations (`type X = capability E(..) where C;`)

Use this flow when changes touch `parse_capability_def` / `resolve_capability_params`
in `mumei-core/src/parser/item.rs`, `TypeRef::carries_effects()` in `ast.rs`, the
capability-constraint branch of `verification/translator/expr.rs`, or the
`Item::CapabilityDef` arms in `src/pipeline.rs` / `commands/check.rs` /
`commands/build.rs`.

Fixtures: `tests/test_capability_stage1.mm` (3 atoms, all must verify) and
`tests/test_capability_stage1_missing_effect.mm` (must exit 1 with
`Effect polymorphism violation: ... accepts capability parameter 'cap' with effect
[SafeFileRead]`, `report.json` `failure_type == "effect_not_allowed"`).

- `mumei check` / `mumei build` print `🔑 Capability: '<Name>' (effect: <Effect>)`.
  That line is the cheapest proof the declaration parsed as a capability rather
  than a refined type.
- BEFORE/AFTER discriminator: on a build without the feature the same file fails
  with `failed to lower refinement type 'FileCap': Unknown function: <Effect>`
  (the `type X = ...` is parsed as a refinement), so a base-binary run via
  `git worktree add -f /home/ubuntu/mumei-base origin/develop` is a strong check.
- To prove the capability `where` constraint is really threaded into Z3, declare
  the underlying effect **without** a constraint and put the constraint only on
  the capability:
  `effect LooseFileRead(path: Str);` +
  `type StrictCap = capability LooseFileRead(path: Str) where starts_with(path, "/tmp/");`
  A body with `let path = "/etc/passwd"; perform cap.read(path);` must fail with
  `Verification Error: Contradiction found.`, while `"/tmp/ok.log"` verifies.
  Without the constraint plumbing both cases verify identically.
- Known limitation (pre-existing for plain effect constraints too, reproducible on
  `origin/develop` with a constrained `effect`): a fully symbolic, unconstrained
  argument (`perform cap.read(user_path)` where `user_path: Str` has no `requires`)
  **verifies** — constant/derived paths are checked, free symbolic ones are not.
  Do not report this as a capability regression; compare against the equivalent
  `perform <Effect>.op(sym)` form first.
- String-literal safety of the `perform cap.op` textual rewrite is best proven
  through emitted IR, not stdout: build with `--emit llvm-ir` and grep the `.ll`
  for the literal, e.g. `grep -o 'c"[^"]*"' out_<atom>.ll` must still contain
  `c"perform cap.read(/tmp/x)\00"` (never `perform <Effect>.read`).
- Two capability types over the **same** effect with different constraints are
  conjoined, so such an atom fails with `Contradiction found` even when only one
  capability is performed. This is a documented Stage 1 over-restriction.
- `capability` is contextual: only a keyword directly after `type X =`. Regression
  fixture worth keeping: an atom with params literally named `capability` and
  `grant`, plus `type capability_alias = i64 where v >= 0;` — must verify.
- `mumei build --output <dir>/<base>` fails with
  `Codegen Error: No such file or directory` when `<dir>` does not exist **and**
  verification was served from cache (`Verification: Skipped (unchanged, cached)`).
  Pre-existing on `origin/develop`; `mkdir -p` the output dir before building.

## Toolchain installer (`mumei setup`) end-to-end testing

Use this flow when changes touch `src/setup.rs` (Z3/LLVM version pins, `Z3Build`
archive suffixes, `select_z3_build`, `detect_host_glibc`/`parse_glibc_version`,
`generate_env_script`, `verify_installation`) or the `$z3Version` pin in
`.github/workflows/release.yml`.

`mumei setup` writes the **real** `~/.mumei`, which also holds toolchains other
workflows use. Check whether it exists first and restore it afterwards (on a
fresh box it is usually absent — then just `rm -rf ~/.mumei` at the end).

### Sandbox runs with `$HOME` instead of touching the real toolchain

There is **no** `MUMEI_HOME` env var. `manifest::mumei_home()` is
`dirs::home_dir()/.mumei`, and `dirs::home_dir()` honours `$HOME`, so each
adversarial case can get its own throwaway home:

```bash
mkdir -p /tmp/mumei-home-case/.mumei/toolchains
# symlink the already-downloaded LLVM so the ~700MB download is skipped
ln -s ~/.mumei/toolchains/llvm-18.1.8 /tmp/mumei-home-case/.mumei/toolchains/llvm-18.1.8
HOME=/tmp/mumei-home-case ./target/debug/mumei setup
```
Do one real-`$HOME` run first so a real LLVM dir exists to symlink; otherwise
every sandbox case re-downloads hundreds of MB.

### Forcing any libc verdict by shadowing `ldd` on PATH

`detect_host_glibc()` shells out to `Cmd::new("ldd").arg("--version")`, resolved
through PATH, so a stub script is the cheapest way to reach branches this host
can't reach naturally:

```bash
mkdir -p /tmp/fakebin && printf '#!/bin/sh\necho "ldd (GNU libc) 2.17"\n' > /tmp/fakebin/ldd
chmod +x /tmp/fakebin/ldd
HOME=/tmp/mumei-home-old PATH=/tmp/fakebin:$PATH ./target/debug/mumei setup
```
Cases worth covering: musl-style output (`musl libc (x86_64)`) → undetectable
libc branch; a version below every archive floor (2.17) → host rejected, and
assert **no** `z3-*` dir is created and the generated env exports no
`Z3_SYS_Z3_HEADER`/`Z3_SYS_Z3_LIB_DIR`/`CPATH`/`LD_LIBRARY_PATH`; and a *lying*
high version (2.40 on an older host) → forces the newest archive, which then
fails to exec.

### Assert env-script paths exist, don't just eyeball them

Upstream Z3 ZIPs ship `libz3.{so,dylib,a}` in **`bin/`**, not `lib/` — true for
4.13.4, 4.14.1 and 5.1.0 alike — so any `<z3>/lib` in the generated env is a
dead path. Source the script and stat everything:

```bash
bash -c '. ~/.mumei/env
  for p in "$Z3_SYS_Z3_HEADER" "$Z3_SYS_Z3_LIB_DIR" "$Z3_SYS_Z3_LIB_DIR/libz3.so"; do
    [ -e "$p" ] && echo "EXISTS  $p" || echo "MISSING $p"; done'
```
The script appends to `$CPATH`/`$LDFLAGS` etc. Since #524 every such export uses
`${VAR:-}`, so sourcing under `set -u` must succeed; if it dies with
`CPATH: parameter not set` on a current branch, that is a product regression
(on builds predating #524 it is expected).

### Prove the loader actually picks the toolchain libz3

Distro Z3 and toolchain Z3 differ in version, which makes a decisive probe:

```bash
cat > /tmp/probe_z3.py <<'PY'
import ctypes
lib = ctypes.CDLL("libz3.so")
lib.Z3_get_full_version.restype = ctypes.c_char_p
print("version:", lib.Z3_get_full_version().decode())
print("resolved:", [l.split()[-1] for l in open("/proc/self/maps") if "libz3" in l][0])
PY
env -u LD_LIBRARY_PATH python3 /tmp/probe_z3.py     # expect the system libz3
bash -c '. ~/.mumei/env; python3 /tmp/probe_z3.py'  # expect the toolchain libz3
```
Running only the second half proves nothing — without the unset-baseline you
cannot tell a working loader export from a coincidence.

### Validating archive-name pins without installing anything

Every `(version, arch, suffix)` combination the code can emit should be
HEAD-checked, **plus a deliberately wrong suffix as a negative control** (GitHub
returns 404 for missing assets, so a control proves the 200s mean something):

```bash
curl -sIL -o /dev/null -w '%{http_code}\n' \
  https://github.com/Z3Prover/z3/releases/download/z3-5.1.0/z3-5.1.0-x64-glibc-2.39.zip
```
Archive **names** are build-image names and are not the real floor: verify the
actual requirement with `objdump -T <libz3.so> | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1`
(5.1.0's x64 archive is named `glibc-2.39` but only imports up to `GLIBC_2.38`).
Check `GLIBCXX_`/`CXXABI_` too when a host fails to load a lib that glibc alone
says should work.

### Host prerequisites that make a real end-to-end `setup` run possible

Real downloads are fast (Z3 ~50 MB, LLVM ~1 GB, seconds each), so prefer a real
run over stubs — but three host details decide whether it can succeed:

- Put sandbox homes on a **disk-backed** path (e.g. `/home/<user>/...`), not
  `/tmp`: `/tmp` is often a ~4 GB tmpfs and the extracted LLVM tree overflows
  it, producing `tar: ... Cannot write: No space left on device`.
- A minimal `PATH` harness must include **`xz`** (and `gzip`), otherwise
  `extract_tar_xz` dies with `tar (child): xz: Cannot exec`.
- Upstream `clang+llvm-18.1.8-x86_64-linux-gnu-ubuntu-18.04` `bin/llc` needs
  `libtinfo.so.5`, which Ubuntu 22.04+ does not ship (`libtinfo.so.6` is not
  ABI-compatible — symlinking it fails with
  `NCURSES_TINFO_5.0.19991023 not found`). Install it with
  `sudo apt-get install -y libtinfo5` before expecting a healthy LLVM verdict.
  Without it a *correct* installer legitimately reports LLVM unusable.

### PATH harnesses to force network / system-z3 branches deterministically

Build two directories of symlinks and use `env -i HOME=... PATH=...` so the run
is hermetic:

```bash
mkdir -p /tmp/mfb-net /tmp/mfb-nonet
for t in curl unzip tar xz gzip ldd; do
  ln -sf $(command -v $t) /tmp/mfb-net/$t; ln -sf $(command -v $t) /tmp/mfb-nonet/$t
done
rm -f /tmp/mfb-nonet/curl
printf '#!/bin/sh\necho "curl: (6) Could not resolve host" >&2\nexit 6\n' > /tmp/mfb-nonet/curl
chmod +x /tmp/mfb-nonet/curl
```

- Neither dir contains `z3`, so `report_version("Z3 (system)", "z3")` fails →
  the "no bundled and no system Z3" branch. Append `:/usr/bin:/bin` to get a
  working system `z3` instead (that is how you reach "no bundle + system z3 →
  exit 0").
- The failing-`curl` dir is the cheap way to make an install attempt fail
  without waiting on the network, i.e. to force `z3_dir = None`.
- Symlink an already-downloaded `llvm-18.1.8` into each sandbox
  `toolchains/` (`install_llvm` only checks `exists()`), so only Z3 is
  re-downloaded per case. A fabricated `llvm-18.1.8/bin/llc` shell stub
  (`exit 1` vs. `echo` a version) is the way to test the LLVM-unusable branch.

### Exit-code and env-file contract after #524

Since #524 (`z3_install_is_usable`, `InstallationStatus`) the exit code is
meaningful, so assert on it as well as on the lines:

- Usable bundled Z3 + runnable `llc` → exit 0, `🎉 Setup complete!`.
- No bundled Z3 but working system `z3` on PATH → still exit 0, plus
  `✅ Z3 (system): ...` and `ℹ️  Using system z3 because no bundled toolchain is
  present`; the env file carries
  `# Z3: no bundled toolchain on this host — using the system install` and no
  `Z3_SYS_Z3_*`.
- `llc --version` not runnable, or neither bundled nor system Z3 → exit **1**
  with `❌ Setup incomplete:` and one `   - ...` line per problem.
- A pre-existing partial (`bin/z3` only) or broken (`bin/z3` exits 1) tree must
  never print `already installed`; it is deleted and reinstalled, and if the
  reinstall fails no Z3 paths are exported.
- Every append-style export uses `${VAR:-}`, so
  `env -u CPATH -u LIBRARY_PATH -u LD_LIBRARY_PATH -u LDFLAGS -u CPPFLAGS \
   sh -euc '. <home>/.mumei/env && echo OK'` must print `OK`. Always pair this
  with the control `sh -euc 'echo "$CPATH"'` (must fail `parameter not set`),
  otherwise the `OK` proves nothing.
- Staging is per-process (`z3-{pid}.zip`, `.staging-{pid}-z3`), and a lost
  rename race is tolerated when the destination validates. Two parallel runs
  against one `$HOME` must both exit 0, never print `Failed to move`, and leave
  no `.staging-*` / `*.zip` behind. Sample `ls -a toolchains/` every 0.1 s in a
  background loop to actually capture the two distinct pid-named artifacts.

A `git worktree` build of the base branch is the strongest control here: before
#524 all of the broken fixtures above exit 0 with `🎉 Setup complete!` and the
generated env fails under `set -u`.

### Regression: `verify` must be unaffected by installer changes

`setup.rs` is not on the verification path, but prove it rather than assume:
run all `std/*.mm` + `examples/*.mm` through both the branch binary and a
`git worktree` build of the base, clearing `.mumei`/`.mumei_cache`/
`.mumei_build_cache` (repo root **and** `std/`, `examples/`) before every single
file, then diff the two `<file> exit=<code>` tables. On 0.6.16 twelve files fail
identically on both sides — including `examples/libc_demo.mm` with exit **101**
(a panic, not a verification failure) — so "some files fail" is expected; only a
*difference* between the tables is a regression.
