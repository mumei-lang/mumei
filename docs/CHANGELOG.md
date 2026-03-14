# 📝 Changelog

---

## Four-Task Implementation: Parser Migration + Extern Codegen + Span Fix + MIR Foundation

### Summary

Implements four interconnected roadmap tasks as a single cohesive change: item parser regex→recursive descent migration, LLVM codegen for extern functions, import span mismatch fix, and MIR foundation with LinearityCtx wiring.

### Task 1: Item Parser regex → Recursive Descent

- **`src/parser/item.rs`**: Rewrote `parse_module_from_source()` and `parse_atom_from_source()` to use token-based parsing via `Lexer` + `ParseContext` instead of ~20 `Regex::new()` calls
- Smart token-to-text reconstruction with `append_token()` helper for context-aware spacing
- All 134 existing tests pass unchanged — backward compatible

### Task 2: LLVM Codegen for Extern Functions (P1-A Completion)

- **`src/codegen.rs`**: Added `declare_extern_functions()` that emits LLVM IR `declare` statements for each `ExternFn`
- Maps Mumei types → LLVM types via `resolve_param_type()`
- Sets calling convention based on `extern_block.language` ("C" or "Rust")
- **`src/main.rs`**: Added `collect_extern_blocks()` helper to gather `ExternBlock` items for codegen
- **`docs/ROADMAP.md`**: Marked P1-A LLVM codegen as ✅

### Task 3: Import Span Mismatch Fix

- **`src/main.rs`**: Created `resolve_source_for_span()` helper that checks `Span.file` and reads the imported file when needed
- Fixed all 5+ locations where `e.with_source(&source, &atom.span)` used the wrong source for imported atoms/impls
- Removed all TODO comments related to the span mismatch bug

### Task 4a: LinearityCtx Wiring into Verification Pipeline

- **`src/verification.rs`**: Added `linearity_ctx: Option<&'a RefCell<LinearityCtx>>` field to `VCtx` struct
- Wrapped `LinearityCtx` in `RefCell` for interior mutability (avoids refactoring 60+ call sites)
- Wired `check_alive()` into `expr_to_z3` Variable branch (use-after-consume detection)
- Wired `borrow()` into call-site `ref`/`ref mut` argument handling
- Wired `consume()` into call-site `consumed_params` argument handling
- `LinearityCtx.borrow()`, `check_alive()`, `consume()` are no longer dead code

### Task 4b: MIR Data Structures + HIR → MIR Lowering

- **`src/mir.rs`** (new): MIR data structures (`Local`, `Place`, `Rvalue`, `Operand`, `MirConstant`, `MirStatement`, `Terminator`, `BasicBlock`, `MirBody`, `LocalDecl`)
- `lower_hir_to_mir()`: Flattens nested HIR expressions into three-address code across BasicBlocks
  - `HirStmt::Let` → `MirStatement::Assign` + `StorageLive`
  - `HirExpr::BinaryOp` → temp + `Rvalue::BinaryOp`
  - `HirExpr::IfThenElse` → 3+ BasicBlocks with `Terminator::SwitchInt`
  - `HirStmt::While` → loop header / body / after blocks with back-edge
  - `HirExpr::Call` → `Rvalue::Call`
- 6 unit tests covering addition, if/else, let binding, function call, while loop, constants
- **`src/hir.rs`**: Updated TODO comment to reference `src/mir.rs`
- **`src/main.rs`**: Added `mod mir;`

### Files Changed

| File | Summary |
|---|---|
| `src/parser/item.rs` | Regex → recursive descent migration with smart token reconstruction |
| `src/codegen.rs` | `declare_extern_functions()` for LLVM IR extern declarations |
| `src/main.rs` | `resolve_source_for_span()`, `collect_extern_blocks()`, `mod mir;` |
| `src/verification.rs` | LinearityCtx wired into VCtx via RefCell, borrow/consume/check_alive at call sites |
| `src/mir.rs` | **New** — MIR data structures + HIR → MIR lowering + 6 unit tests |
| `src/hir.rs` | Updated MIR TODO comment |
| `docs/ROADMAP.md` | P1-A LLVM codegen ✅, Phase 4 status updated |
| `docs/ARCHITECTURE.md` | Pipeline diagram + source file table updated |
| `docs/CHANGELOG.md` | This entry |

### Test Results

- 140 tests passing (134 original + 6 new MIR tests)

---

## PR #62: Parser Migration — Recursive Descent with Proper Lexer

### Summary

Migrates the parser from regex-based approach to a full recursive descent parser with proper lexer. Replaces monolithic `src/parser.rs` (3,052 lines) with 7 focused modules under `src/parser/`. Also incorporates PR #61's `contract()` clause parsing for higher-order function parameters.

### Parser Module Structure

| Module | Role |
|---|---|
| `src/parser/mod.rs` | Public API, `ParseContext` struct, 84+ tests |
| `src/parser/token.rs` | `Token` enum (60+ variants), `SpannedToken` with line/col/len |
| `src/parser/lexer.rs` | `Lexer` — source string → `Vec<SpannedToken>` with span tracking |
| `src/parser/ast.rs` | All AST types (`Expr`, `Stmt`, `Item`, `Atom`, etc.) |
| `src/parser/expr.rs` | Pratt parser for expressions (operator precedence via binding power) |
| `src/parser/item.rs` | Recursive descent for top-level items (replaces ~15 regex patterns) |
| `src/parser/pattern.rs` | Match arm pattern parsing |

### Key Changes

- **Lexer**: Proper tokenization with span tracking (line/col/len per token), handles comments, string literals, multi-character operators (`==`, `!=`, `>=`, `<=`, `=>`, `&&`, `||`, `|>`, `->`)
- **Pratt Parser**: Extensible operator precedence via binding power table — trivial to add `|>`, `@`, future operators
- **Item Parsing**: All top-level items (import, type, struct, enum, trait, impl, resource, effect, extern, atom) parsed via recursive descent instead of regex
- **contract() Clause**: `Param.fn_contract_requires` and `Param.fn_contract_ensures` fields for higher-order function parameter contracts (from PR #61)
- **Keyword Field Access**: Keywords like `mode`, `priority` correctly handled as field names after `.` and as function names in expression contexts
- **Backward Compatible**: All public APIs preserved (`parse_module`, `parse_expression`, `parse_body_expr`, `parse_atom`, `tokenize`) — zero caller changes needed

### Unblocks

- Phase 2: Basic Effect System (`<E: Effect>` generic syntax)
- Phase C: Lambda syntax (`fn(x) => x + 1`)
- MIR introduction (CFG with accurate source spans)

---

## PR #61: call_with_contract — Z3 Verification of Higher-Order Functions

### Summary

Implements `call_with_contract` in the verification engine so that higher-order functions (`map`, `fold_left`, `result_map`, etc.) can be formally verified by Z3 without `trusted` markers.

### Key Changes

- **`contract(f)` clause syntax**: Declare requires/ensures constraints for function parameters
- **Phase B verification**: `CallRef` dynamic case expands contracts via Z3 (requires validation + ensures assertion)
- **Removed `trusted`** from `map`, `fold_left`, `list_map`, `result_map`, `apply`, `apply_twice`, `fold_two`
- **Documentation**: `instruction.md` §3.5 — contract syntax reference

### Test Results

- `tests/test_call_with_contract.mm`: 10/10 atoms verified
- `std/option.mm`: 8/8 verified (including `map` without `trusted`)
- `std/result.mm`: 12/12 verified (including `result_map` without `trusted`)
- `std/list.mm`: `fold_left` and `list_map` verified without `trusted`

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

- `.mumei/cache/verification_cache.json` with enhanced per-atom verification cache
- `compute_proof_hash()`: hashes `name | requires | ensures | body_expr | consume | ref | effects | trust | callee signatures | type predicates`
- Transitive dependency tracking: callee contract changes automatically invalidate callers
- `VerificationCacheEntry`: stores `proof_hash`, `result`, `dependencies`, `type_deps`, `timestamp`
- Old `.mumei_build_cache` automatically migrated via `migrate_old_cache()`
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
