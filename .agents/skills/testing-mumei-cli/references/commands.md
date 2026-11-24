# Mumei CLI command and fixture reference

Read this when you need the exact CLI invocation for a flag/subcommand or want to pick an existing `.mm` fixture instead of writing one.

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
