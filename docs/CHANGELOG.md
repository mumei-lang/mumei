# 📝 Changelog

---

## PR #32: Strategic Roadmap v0.3.0+ — Full Implementation (P1 + P2 + P3)

### Summary

PR #31 で定義した戦略的ロードマップの全 3 優先順位を実装。
ネットワーク・ファースト標準ライブラリ、ランタイムポータビリティ、CLI ツールの完全実装。

### Implementation Highlights

| Priority | Phase | Implementation |
|---|---|---|
| P1-A | FFI Bridge | `src/main.rs` + `src/resolver.rs`: extern → trusted atom 自動登録 |
| P1-B | std.json | `std/json.mm`: 19 atoms (parse, stringify, get, array, object) |
| P1-C | std.http | `std/http.mm`: 11 atoms (get, post, put, delete, status, body) + reqwest 依存追加 |
| P1-D | Integration Demo | `examples/http_json_demo.mm`: task_group + HTTP + JSON 並行処理 |
| P2-A | CI Portability | `release.yml`: LLVM 17 apt セットアップ + 依存ライブラリ追加 (aarch64-linux は将来対応) |
| P2-B | Homebrew | `scripts/homebrew/mumei.rb`: Formula テンプレート |
| P2-C | WebInstall | `scripts/install.sh`: curl \| sh インストーラー |
| P3-A | REPL | `src/main.rs`: `mumei repl` コマンド (対話的実行環境) |
| P3-B | Doc Gen | `src/main.rs`: `mumei doc` コマンド (HTML/Markdown 自動生成) |
| P3-C | Integration | REPL 内で `:load std/http.mm` → HTTP atoms 利用可能 |

### Files Changed

| File | Summary |
|---|---|
| `src/main.rs` | FFI Bridge, `mumei repl`, `mumei doc` コマンド追加 |
| `src/resolver.rs` | ExternBlock → trusted atom 登録 (import 経由) |
| `Cargo.toml` | inkwell 修正 (0.5.0), reqwest 追加 |
| `std/json.mm` | **New** — JSON 操作標準ライブラリ (19 atoms) |
| `std/http.mm` | **New** — HTTP クライアント標準ライブラリ (11 atoms) |
| `examples/http_json_demo.mm` | **New** — task_group + HTTP + JSON 統合デモ |
| `scripts/install.sh` | **New** — curl \| sh インストーラー |
| `scripts/homebrew/mumei.rb` | **New** — Homebrew Formula テンプレート |
| `.github/workflows/release.yml` | LLVM 17 apt セットアップ + 依存ライブラリ追加 |
| `docs/STDLIB.md` | std.json, std.http リファレンス更新 |
| `docs/ROADMAP.md` | ステータスを Implemented に更新 |
| `docs/CHANGELOG.md` | 今回の変更を記録 |

---

## PR #31: Strategic Roadmap v0.3.0+ (docs update)

### Summary

Mumei を「実験的言語」から「実用ツール」へ昇華させる 3 つの戦略的ロードマップを定義。
全関連ドキュメントを更新し、優先順位・依存関係・タイムラインを明確化。

### 3 Strategic Priorities

| Priority | Theme | Key Deliverable |
|---|---|---|
| 🥇 P1 | Network-First Standard Library | FFI Bridge + std.json + std.http |
| 🥈 P2 | Runtime Portability | Static linking + Homebrew + WebInstall |
| 🥉 P3 | CLI Developer Experience | mumei repl + mumei doc |

### Files Changed

| File | Summary |
|---|---|
| `docs/ROADMAP.md` | **New** — 詳細な戦略的ロードマップ (Phase A–D, 依存関係, Success Metrics, Timeline) |
| `README.md` | Roadmap セクションに std.json, Runtime Portability, REPL, doc gen 追加 |
| `instruction.md` | §11 を Strategic Roadmap v0.3.0+ に書き換え (3 priorities) |
| `docs/TOOLCHAIN.md` | Future Roadmap を 3 プライオリティのテーブル形式に更新 |
| `docs/FFI.md` | 将来の拡張に FFI Bridge Completion の実装計画を追加 |
| `docs/CONCURRENCY.md` | 将来の拡張に std.http 統合デモ + Task 洗練項目追加 |
| `docs/STDLIB.md` | Planned: std/json.mm + std/http.mm セクション追加 |
| `docs/CHANGELOG.md` | 今回の変更を記録 |

---

## PR #16 (feature/alloc → develop)

### Summary

This PR implements dynamic memory management, ownership system, borrowing, and completes the remaining roadmap items (except LSP) for the Mumei language.

---

## Phase 1–3: Standard Prelude Foundation

- **`std/prelude.mm`**: `Eq`/`Ord`/`Numeric` traits with Z3 laws, `Option<T>`/`Result<T,E>`/`List<T>`/`Pair<T,U>` ADTs, `Sequential`/`Hashable` abstract interfaces
- **`src/resolver.rs`**: `resolve_prelude()` for auto-import
- **`src/main.rs`**: Prelude auto-loading in `load_and_prepare()`

## Phase 4: Trait Method Refinement Constraints

- `TraitMethod.param_constraints` field in `src/parser.rs`
- Syntax: `fn div(a: Self, b: Self where v != 0) -> Self;`
- `Numeric` trait gains `div` with zero-division prevention

## Phase 5: Law Body Expansion

- `substitute_method_calls()` in `src/verification.rs`
- Word-boundary-aware `replace_word()` substitution
- `split_args()` for nested parenthesis handling
- Error messages now show expanded law expressions

## Phase 6: Dynamic Memory (alloc)

- **`std/alloc.mm`**: `RawPtr`, `NullablePtr`, `Owned` trait, `Vector<T>`, `HashMap<K,V>`
- **`src/verification.rs`**: `LinearityCtx` — ownership + borrowing tracking
- **`src/codegen.rs`**: `alloc_raw` → `malloc`, `dealloc_raw` → `free` (LLVM IR)

## Ownership & Borrowing

- **`consume` modifier**: `Atom.consumed_params` parsed from `consume x;` syntax
- **`ref` keyword**: `Param.is_ref` parsed from `ref v: T` syntax
- **Z3 integration**: `__alive_` / `__borrowed_` symbolic Bools
- **LinearityCtx**: `register()`, `consume()`, `borrow()`, `release_borrow()`, `check_alive()`
- **Transpiler**: Rust `ref` → `&T`, TypeScript `ref` → `/* readonly */`

## HashMap\<K, V\>

- `struct HashMap<K, V> { buckets, size, capacity }` with field constraints
- 11 verified atoms: `map_new`, `map_insert`, `map_get`, `map_contains_key`, `map_remove`, `map_size`, `map_is_empty`, `map_rehash`, `map_drop`, `map_insert_safe`, `map_should_rehash`

## Equality Ensures Propagation

- `ensures: result == n + 1` now propagates through chained calls
- `propagate_equality_from_ensures()` recursively extracts `result == expr` from `&&`-joined ensures

## FQN Dot-Notation

- `math.add(x, y)` resolved as `math::add` in both verification and codegen
- Automatic `.` → `::` conversion

## Incremental Build

- `.mumei_build_cache` with per-atom SHA-256 hashing
- `compute_atom_hash()`: hashes `name | requires | ensures | body_expr | consume | ref`
- Unchanged atoms skip Z3 verification
- Cache invalidation on verification failure

## Nested Struct Support

- `v.point.x` resolved via recursive `build_field_path()`
- Path flattening: `["v", "point", "x"]` → `v_point_x` / `__struct_v_point_x`
- LLVM codegen: recursive `extract_value` chains

## Struct Method Definitions

- `StructDef.method_names` field for FQN registration as `Stack::push`

## Negative Test Suite

8 test files in `tests/negative/`:

| File | Tests |
|---|---|
| `postcondition_fail.mm` | ensures violation |
| `division_by_zero.mm` | zero-division detection |
| `array_oob.mm` | out-of-bounds access |
| `match_non_exhaustive.mm` | non-exhaustive match |
| `consume_ref_conflict.mm` | ref + consume conflict |
| `invariant_fail.mm` | loop invariant initial failure |
| `requires_not_met.mm` | inter-atom precondition violation |
| `termination_fail.mm` | non-decreasing ranking function |

---

## Files Changed

| File | Summary |
|---|---|
| `std/prelude.mm` | Traits, ADTs, interfaces, alloc reference |
| `std/alloc.mm` | **New** — Vector, HashMap, ownership primitives |
| `src/parser.rs` | `param_constraints`, `consumed_params`, `is_ref`, `method_names` |
| `src/verification.rs` | LinearityCtx, law expansion, equality propagation, nested struct, FQN |
| `src/codegen.rs` | malloc/free, FQN dot-notation, nested extract_value |
| `src/resolver.rs` | Prelude auto-load, incremental build cache |
| `src/main.rs` | Prelude integration, incremental build in verify/build |
| `src/transpiler/rust.rs` | `ref` → `&T` |
| `src/transpiler/typescript.ts` | `ref` → `/* readonly */` |
| `tests/negative/*.mm` | 8 negative test files |
| `README.md` | Full documentation update |
| `docs/STDLIB.md` | **New** — Standard library reference |
| `docs/CHANGELOG.md` | **New** — This file |

---

## Remaining Roadmap (pipeline integration pending)

The following data structures and logic are implemented but not yet wired into the compiler pipeline:

| Item | Data Structure | Missing Integration |
|---|---|---|
| Struct method parsing | `StructDef.method_names` | Parser for `impl Stack { atom push(...) }` syntax |
| Trait method constraints | `TraitMethod.param_constraints` | Z3 injection in `verify_impl` and inter-atom calls |
| Automatic borrow tracking | `LinearityCtx.borrow()` / `release_borrow()` | Call-site `ref` arg → borrow registration in `expr_to_z3` |
| Use-after-consume detection | `LinearityCtx.check_alive()` | Variable access check in `expr_to_z3` `Variable` branch |
