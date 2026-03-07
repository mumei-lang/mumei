# 📝 Changelog

---

## Effect System: Inference, Refinement Types × Effects, Hierarchy

### Summary

Implements comprehensive Effect Inference and Refinement Types × Effects integration with a hybrid verification approach (Constant Folding + Symbolic String ID).

### Effect System Core

- **Effect/EffectDef structs**: `Effect` with params, `EffectDef` with constraint and `parent:` for hierarchy
- **Parser**: `effects: [...]` clause in atoms, `effect` declarations with `parent:` and `where` constraints
- **Effect Hierarchy (Subtyping)**: `parent:` field enables Network → HttpRead/TcpConnect relationships
- **`get_effect_ancestors()` / `is_subeffect()`**: Traverse hierarchy chain for subtype checking

### Effect Inference

- **`infer_effects()`**: Call graph traversal infers required effects from callee atoms
- **`infer_effects_json()`**: JSON serialization for CLI/MCP integration
- **`verify_effect_consistency()`**: Checks declared vs inferred effects with subtyping support

### Hybrid Path Verification

- **Constant Folding**: Rust-side compile-time check for literal path constraints (e.g., `starts_with`)
- **Symbolic String ID**: Path strings mapped to Z3 Int sort for variable path verification
- **`verify_effect_params()`**: Effect parameter constraint verification

### CLI & MCP

- **`mumei infer-effects <file>`**: New CLI subcommand for JSON effect inference output
- **`get_inferred_effects` MCP tool**: Pre-check tool for AI to verify required permissions before writing code

### File Consistency

- All transpilers (Rust/Go/TypeScript) output effect annotations as doc comments
- LLVM IR codegen includes effect metadata
- LSP hover displays effect information
- Resolver handles `Item::EffectDef` registration with FQN support
- `compute_atom_hash()` includes effect fields for cache invalidation

### Documentation

- **ROADMAP.md**: Z3 String Sort migration plan + Effect hierarchy extensions
- **ARCHITECTURE.md**: Updated verification steps (1f, 1g) and pipeline diagram
- **instruction.md**: Updated coding conventions for `Item::EffectDef` and `Atom.effects`

---

## PR #35: miette Rich Diagnostics (Phase 1) + Higher-Order Functions `atom_ref` (Phase 2)

### Summary

Replaces plain-text error output with [miette](https://crates.io/crates/miette)-powered rich diagnostics (colored output, source code context, underline highlighting, actionable suggestions). Introduces first-class function references via `atom_ref(name)` and indirect invocation via `call(f, args...)` with automatic contract propagation through Z3.

### Phase 1: miette Integration

- `MumeiError` now derives `thiserror::Error` + `miette::Diagnostic` with `NamedSource`, `SourceSpan`, and `#[help]`
- `span_to_source_span()` converts line/col/len `Span` → byte-offset `SourceSpan` (handles `\n` and `\r\n`)
- Builder methods `.with_source()` and `.with_help()` for post-hoc error enrichment
- `original_span: Span` field preserved for LSP backward compatibility
- Help suggestions for: postcondition violations, precondition violations, division by zero, out-of-bounds, refinement type predicates

### Phase 2: Higher-Order Functions (Phase A)

- **AST**: `Expr::AtomRef { name }` and `Expr::CallRef { callee, args }` variants
- **Parser**: `atom_ref(name)` and `call(expr, args...)` in `parse_primary()`; depth-tracking parenthesis parser for nested `atom_ref(i64, i64) -> i64` in parameter lists
- **Verification**: `CallRef` resolves `atom_ref(concrete_name)` contracts; parametric function-type parameters return unconstrained symbolic values (atoms must be `trusted`)
- **Codegen**: `AtomRef` → function pointer via `ptr_to_int` with lazy forward declaration; `CallRef` → direct call optimization or `build_indirect_call`
- **Transpilers**: Function type mapping for Rust (`fn(T) -> R`), Go (`func(T) R`), TypeScript (`(arg: number) => number`)
- **Std library**: `map` (option.mm), `fold_left`/`list_map` (list.mm), `result_map` (result.mm) — all `trusted` for Phase A
- **Example**: `examples/higher_order_demo.mm`

### Bug Fixes

- Fixed `to_detail()` LSP regression — `original_span` field restores `Span` propagation
- Fixed `parse_atom` regex `[^)]*` — replaced with depth-tracking parenthesis parser
- Fixed `collect_callees` missing `AtomRef`/`CallRef` match arms (cycle detection)
- Fixed `count_self_calls` missing `AtomRef`/`CallRef` match arms (async recursion depth)
- Fixed `collect_acquire_resources` missing `AtomRef`/`CallRef` match arms (BMC resource safety)
- Fixed `body_contains_float` missing recursion into `CallRef` args (Rust transpiler)
- Fixed codegen forward reference — `AtomRef` lazily declares functions not yet in LLVM module
- Fixed indirect call f64 type — inspects actual argument types via `is_float_value()`

### Files Changed

| File | Summary |
|---|---|
| `src/verification.rs` | `MumeiError` restructured with miette derives, `span_to_source_span`, `original_span`, `AtomRef`/`CallRef` in `expr_to_z3`/`collect_callees`/`count_self_calls` |
| `src/parser.rs` | `AtomRef`/`CallRef` expr variants, depth-tracking param parser, `split_params`, fn type parsing |
| `src/codegen.rs` | `AtomRef` → `ptr_to_int` with lazy declare, `CallRef` → direct/indirect call |
| `src/main.rs` | miette handler init, `load_and_prepare` returns source, `with_source` at all error sites |
| `src/ast.rs` | `TypeRef::fn_type()`, `is_fn_type()`, monomorphizer `AtomRef`/`CallRef` traversal |
| `src/transpiler/*.rs` | Function type mapping, `AtomRef`/`CallRef` formatting |
| `src/lsp.rs` | `to_detail()` now uses `original_span` for LSP positioning |
| `Cargo.toml` | Added `miette` (v7, fancy) and `thiserror` (v2) |
| `std/option.mm` | `trusted atom map` with `atom_ref` parameter |
| `std/result.mm` | `trusted atom result_map` with `atom_ref` parameter |
| `std/list.mm` | `trusted atom fold_left`/`list_map` with `atom_ref` parameters |
| `examples/higher_order_demo.mm` | **New** — `atom_ref` + `call` demonstration |
| `docs/DIAGNOSTICS.md` | Rewritten for miette integration (English) |
| `README.md` | Higher-order functions section, rich diagnostics showcase, roadmap updates |

---

## PR #32: Strategic Roadmap v0.3.0+ — Full Implementation (P1 + P2 + P3)

### Summary

Implements all 3 priorities from the strategic roadmap defined in PR #31.
Network-first standard library, runtime portability, and CLI tools.

### Implementation Highlights

| Priority | Phase | Implementation |
|---|---|---|
| P1-A | FFI Bridge | `src/main.rs` + `src/resolver.rs`: extern → trusted atom auto-registration |
| P1-B | std.json | `std/json.mm`: 19 atoms (parse, stringify, get, array, object) |
| P1-C | std.http | `std/http.mm`: 11 atoms (get, post, put, delete, status, body) + reqwest dependency |
| P1-D | Integration Demo | `examples/http_json_demo.mm`: task_group + HTTP + JSON parallel processing |
| P2-A | CI Portability | `release.yml`: LLVM 17 apt setup + dependency libraries (aarch64-linux planned) |
| P2-B | Homebrew | `scripts/homebrew/mumei.rb`: Formula template |
| P2-C | WebInstall | `scripts/install.sh`: curl \| sh installer |
| P3-A | REPL | `src/main.rs`: `mumei repl` command (interactive execution) |
| P3-B | Doc Gen | `src/main.rs`: `mumei doc` command (HTML/Markdown auto-generation) |
| P3-C | Integration | `:load std/http.mm` in REPL → HTTP atoms available |

### Files Changed

| File | Summary |
|---|---|
| `src/main.rs` | FFI Bridge, `mumei repl`, `mumei doc` commands |
| `src/resolver.rs` | ExternBlock → trusted atom registration (via import) |
| `Cargo.toml` | inkwell fix (0.5.0), reqwest added |
| `std/json.mm` | **New** — JSON operations standard library (19 atoms) |
| `std/http.mm` | **New** — HTTP client standard library (11 atoms) |
| `examples/http_json_demo.mm` | **New** — task_group + HTTP + JSON integration demo |
| `scripts/install.sh` | **New** — curl \| sh installer |
| `scripts/homebrew/mumei.rb` | **New** — Homebrew Formula template |
| `.github/workflows/release.yml` | LLVM 17 apt setup + dependency libraries |
| `docs/STDLIB.md` | std.json, std.http reference updated |
| `docs/ROADMAP.md` | Status updated to Implemented |
| `docs/CHANGELOG.md` | This changelog entry |

---

## PR #31: Strategic Roadmap v0.3.0+ (docs update)

### Summary

Defines 3 strategic roadmap priorities to evolve Mumei from an experimental language to a practical tool.
All related documentation updated with priorities, dependencies, and timelines.

### 3 Strategic Priorities

| Priority | Theme | Key Deliverable |
|---|---|---|
| 🥇 P1 | Network-First Standard Library | FFI Bridge + std.json + std.http |
| 🥈 P2 | Runtime Portability | Static linking + Homebrew + WebInstall |
| 🥉 P3 | CLI Developer Experience | mumei repl + mumei doc |

### Files Changed

| File | Summary |
|---|---|
| `docs/ROADMAP.md` | **New** — Detailed strategic roadmap (Phase A–D, dependencies, success metrics, timeline) |
| `README.md` | Added std.json, Runtime Portability, REPL, doc gen to Roadmap section |
| `instruction.md` | §11 rewritten as Strategic Roadmap v0.3.0+ (3 priorities) |
| `docs/TOOLCHAIN.md` | Future Roadmap updated to 3-priority table format |
| `docs/FFI.md` | Added FFI Bridge Completion implementation plan to future extensions |
| `docs/CONCURRENCY.md` | Added std.http integration demo + Task refinement items |
| `docs/STDLIB.md` | Added Planned: std/json.mm + std/http.mm sections |
| `docs/CHANGELOG.md` | This changelog entry |

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
