# Core verification testing flows

Read this when changes touch the Z3 verification path: spurious-counterexample detection, cross-spec reports, contradiction/unsat-core diagnostics, polymorphic arrays, tuple returns, `**`/chained comparisons, the `unverifiable` verdict, or the borrow checker.

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

For certificate metadata, run the Proof Certificate flow in [certificates-emitters-lean.md](certificates-emitters-lean.md#proof-certificate-generation-and-verification) and additionally assert:
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
> The `exit 1` expectations below predate the exit-code contract, which maps `unverifiable` to exit 3; see [notes-exit-codes-json.md](notes-exit-codes-json.md#exit-codejson-contract-testing).
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

### Borrow checker (R-14, `mir_analysis/borrow_check.rs`)

Hard errors surface as `Verification Error: borrow check failed in '<atom>': <msg>`
and exit 1; grep for `borrow check failed`. Writing ad-hoc `.mm` probes is the
fastest way to exercise the rules; mark callees `trusted atom` so Z3 does not add
noise, and `while` loops need an `invariant` clause or you get a syntax error
instead of a borrow error. Clear `.mumei`, `.mumei_build_cache` in the probe dir
between unrelated runs, or use the cache deliberately: verify twice (second run
prints `skipped (unchanged, cached)`), then flip a callee param `ref` -> `ref mut`
and confirm the caller re-verifies (dependency/`VERIFIER_POLICY_VERSION` hashing).
Field borrows are conservative (same root local): `f(ref mut s.a, ref s.b)` is
rejected. Block expressions as call arguments (`g(x, { x = 5; x })`) are not
lowered as blocks — use `if true { ... } else { ... }` in probes instead.
For Rust-level coverage run `cargo test --test test_borrow_check`.
