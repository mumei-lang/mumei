# Proof certificates, external emitters and Lean escalation

Read this when changes touch external `--emit <name>` plugin loading, escalation bundles / the mumei-lean bridge, `--allow-lean-verified`, or proof-certificate generation and `verify-cert`.

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
