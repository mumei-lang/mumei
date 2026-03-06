# 🗡️ Mumei (無銘)

**Mathematical Proof-Driven Programming Language.**

**Mumei (無銘)** is a formally verified language that processes source code through the pipeline:

> parse → resolve (imports) → monomorphize (generics) → verify (Z3) → codegen (LLVM IR) → transpile (Rust / Go / TypeScript)

Only atoms that pass formal verification are compiled to LLVM IR and transpiled to multi-language source code. Every function's preconditions, postconditions, loop invariants, termination, and trait law satisfaction are mathematically proven before a single line of machine code is emitted.

---

## 🔍 Friendly Error Messages

Mumei uses [miette](https://crates.io/crates/miette) for rich, developer-friendly error diagnostics with colored output, source code highlighting, and actionable suggestions.

**Before** (plain text):
```
❌ 'decrement': verification failed: Verification Error: Postcondition (ensures) is not satisfied.
```

**After** (miette rich diagnostics):
```
  × Verification Error: Postcondition (ensures) is not satisfied.
   ╭─[examples/basic.mm:5:1]
 4 │   ensures: result > 0;
 5 │   body: x - 1;
   ·   ──────────── verification failed here
 6 │
   ╰────
  help: Check the ensures condition. Verify that the body's return value satisfies the postcondition.
```

Every verification error now includes:
- **Source location** — exact file, line, and column with source code context
- **Underline highlighting** — visual pointer to where the error occurred
- **Actionable suggestions** — concrete help messages to guide fixes (e.g., "Add `divisor != 0` to requires")

---

## ✨ Features

### Core Language
- **Refinement Types** — `type Nat = i64 where v >= 0;` with Z3-backed predicates
- **Structs / Enums (ADT)** — per-field constraints, pattern matching with Z3 exhaustiveness checking
- **Generics** — monomorphization at compile time (`Pair<T, U>`, `Option<T>`)
- **Trait System with Laws** — algebraic laws verified by Z3 (`law reflexive: leq(x, x) == true`)
- **Loop Invariant + Termination** — `invariant:` + `decreases:` with inductive proof

### Verification
- **Quantifiers in ensures** — `forall(i, 0, n, arr[i] <= arr[i+1])` in postconditions
- **Ownership & Borrowing** — `ref` / `ref mut` / `consume` with Z3 aliasing prevention
- **Async/Await + Resource Hierarchy** — deadlock-free proof via Z3 priority ordering
- **Trust Boundary** — `trusted` / `unverified` atoms with taint analysis
- **BMC + Inductive Invariant** — bounded model checking upgradable to complete proof

### Higher-Order Functions (Phase A)
- **`atom_ref(name)`** — reference an atom as a first-class value (function pointer)
- **`call(f, args...)`** — invoke a referenced atom with automatic contract propagation
- **Function type parameters** — `f: atom_ref(i64) -> i64` in atom signatures

```mumei
atom increment(x: i64)
    requires: x >= 0;
    ensures: result == x + 1;
    body: x + 1;

// Atoms with parametric function-type parameters use `trusted`
// because f's contract is unknown at verification time (Phase B will resolve).
trusted atom apply_twice(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: call(f, call(f, x));

// Usage: apply_twice(5, atom_ref(increment)) → 7
// At this call site, increment's contract IS propagated via atom_ref.
```

> **Known limitation (Phase A):** When `call(f, x)` is used with a function-type *parameter* `f` (not a literal `atom_ref(name)`), the verifier cannot resolve `f`'s contract — it returns an unconstrained symbolic value. Such atoms must be marked `trusted`. Direct `call(atom_ref(concrete_name), x)` works fully with contract propagation. Phase B (`call_with_contract`) will resolve this.

### Standard Library (Verified)
- **Option / Result** — `map`, `map_apply`, `and_then_apply`, `or_else`, `filter`, `wrap_err`
- **List** — immutable ops (`head`/`tail`/`append`/`prepend`/`reverse`) + fold ops (`sum`/`count`/`min`/`max`/`all`/`any`)
- **Sort Algorithms** — `insertion_sort`, `merge_sort`, `binary_search` with termination + invariant proofs
- **Sorted Array Proofs** — `verified_insertion_sort` with `forall` in ensures: `arr[i] <= arr[i+1]`
- **BoundedArray** — push/pop with overflow/underflow prevention, sorted operations
- **Dynamic Memory** — `Vector<T>`, `HashMap<K, V>` with field constraints

### FFI & Concurrency
- **FFI extern blocks** — `extern "Rust" { fn sqrt(x: f64) -> f64; }` with Span tracking
- **Structured Concurrency** — `task { ... }` / `task_group:all { ... }` / `task_group:any { ... }` with Z3 join constraints

### Output
- **Multi-target Transpiler** — Rust + Go + TypeScript (including Task/TaskGroup)
- **LLVM IR Codegen** — Pattern Matrix, StructType, malloc/free, Task/TaskGroup

---

## 🔬 Quick Example

```mumei
type Nat = i64 where v >= 0;

atom increment(n: Nat)
requires: n >= 0;
ensures: result >= 1;
body: { n + 1 };

// Sorted array proof with forall in ensures
trusted atom verified_sort(n: i64)
requires: n >= 0;
ensures: result == n && forall(i, 0, result - 1, arr[i] <= arr[i + 1]);
body: n;
```

> 📖 **Language reference**: [`docs/LANGUAGE.md`](docs/LANGUAGE.md) — types, generics, traits, termination, modules, quantifiers, ownership, async
>
> 📖 **Standard library**: [`docs/STDLIB.md`](docs/STDLIB.md) — Option, Result, List, BoundedArray, sort algorithms, fold operations
>
> 📖 **Examples & tests**: [`docs/EXAMPLES.md`](docs/EXAMPLES.md) — verification suite, pattern matching, inter-atom calls, negative tests

---

## 🛠️ Forging Process

| Stage | Name | Description |
|---|---|---|
| 1 | **Polishing** (Parser) | Parses all definitions including generics, `ref`/`ref mut`/`consume`, `async`/`acquire`/`await`, `trusted`/`unverified`, `invariant`, match with guards |
| 2 | **Resolving** (Resolver) | Import resolution, circular detection, prelude auto-load, incremental cache |
| 3 | **Monomorphization** | Expands `Stack<i64>`, `Stack<f64>` into concrete definitions |
| 4 | **Verification** (Z3) | Trust boundary → resource hierarchy → BMC → async recursion depth → inductive invariant → call graph cycles → contracts → aliasing → taint analysis → ownership/borrowing |
| 5 | **Tempering** (LLVM IR) | Pattern Matrix codegen, StructType, malloc/free, mutex_lock/unlock, nested extract_value |
| 6 | **Sharpening** (Transpiler) | Rust + Go + TypeScript with ownership mapping (`ref` → `&T`, `ref mut` → `&mut T`, `acquire` → lock/unlock) |

> 📖 **Detailed architecture**: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | **Changelog**: [`docs/CHANGELOG.md`](docs/CHANGELOG.md)

---

## 🚀 Quickstart

### Option A: Download pre-built binary (recommended)

Download from [GitHub Releases](https://github.com/mumei-lang/mumei/releases) — no Rust toolchain required.

```bash
# Example: macOS aarch64
curl -LO https://github.com/mumei-lang/mumei/releases/latest/download/mumei-aarch64-apple-darwin.tar.gz
tar xzf mumei-aarch64-apple-darwin.tar.gz
sudo mv mumei /usr/local/bin/
sudo mv std /usr/local/share/mumei-std
export MUMEI_STD_PATH=/usr/local/share/mumei-std
```

### Option B: Build from source

> **Note**: `cargo build --release` compiles the Mumei compiler itself (written in Rust) into a native binary at `target/release/mumei`. This is a one-time step — after building, you use the `mumei` command to work with `.mm` source files.

```bash
# 1. Install system dependencies (macOS)
xcode-select --install
brew install llvm@17 z3

# 2. Build the mumei compiler (one-time)
./build_and_run.sh          # sets env vars + cargo build --release
# Or manually:
cargo build --release       # → target/release/mumei

# 3. Install globally (optional)
cargo install --path .      # → ~/.cargo/bin/mumei

# Alternative: auto-install Z3/LLVM without brew
mumei setup                 # downloads to ~/.mumei/toolchains/
source ~/.mumei/env         # apply env vars
```

### 3 steps to start

```bash
mumei init my_app           # create project (mumei.toml + .gitignore + src/main.mm)
cd my_app
mumei build src/main.mm -o dist/output   # verify + codegen + transpile
```

### CLI Commands

```bash
mumei build input.mm -o dist/katana   # Full pipeline: verify → codegen → transpile
mumei verify input.mm                 # Z3 verification only (no codegen)
mumei check input.mm                  # Parse + resolve only (fast, no Z3)
mumei init my_project                 # Generate project template
mumei add ./libs/math                 # Add path dependency
mumei add https://github.com/user/mm  # Add git dependency
mumei add math_utils                  # Add registry dependency
mumei publish                         # Publish to local registry
mumei publish --proof-only            # Publish proof cache only
mumei setup                           # Download Z3 + LLVM toolchain
mumei inspect                         # Inspect development environment
mumei lsp                             # Start LSP server
```

### Verify your setup

```bash
# Check environment
mumei inspect

# Run the main test suite
mumei build sword_test.mm -o dist/katana

# Run examples
mumei build examples/call_test.mm -o dist/call_test
mumei build examples/match_atm.mm -o dist/match_atm
mumei build examples/match_evaluator.mm -o dist/match_evaluator
mumei build examples/import_test/main.mm -o dist/import_test
mumei build examples/higher_order_demo.mm -o dist/higher_order_demo

# Standard library tests
mumei build tests/test_std_import.mm -o dist/test_std
mumei verify tests/test_forall_ensures.mm

# Negative test (should fail — failure is correct)
mumei verify tests/negative/forall_ensures_fail.mm || echo "OK (expected)"

# Rust unit tests
cargo test
```

### Development Setup (pre-commit hooks)

```bash
pip install pre-commit && pre-commit install
pre-commit run --all-files
```

Hooks: `check-yaml` · `end-of-file-fixer` · `trailing-whitespace` · `cargo fmt` · `cargo clippy` · `cargo test`

### Generated project structure

```bash
mumei init my_app
```

```
my_app/
├── mumei.toml        # Package manifest ([package]/[build]/[proof]/[dependencies])
├── .gitignore        # Ignores dist/, *.ll, .mumei_build_cache, etc.
├── dist/             # Build output directory
└── src/
    └── main.mm       # Entry point with verification examples
```

---

## 📂 Project Structure

```
├── src/
│   ├── main.rs            # CLI orchestrator (build/verify/check/init/add/publish/setup/inspect/lsp)
│   ├── parser.rs          # AST (Span, Expr, Task/TaskGroup, ExternBlock), tokenizer, parser
│   ├── ast.rs             # TypeRef, Monomorphizer (Task/TaskGroup traversal)
│   ├── resolver.rs        # Import resolution, dependency resolution, circular detection
│   ├── verification.rs    # Z3 verification, ModuleEnv, forall/exists
│   ├── codegen.rs         # LLVM IR generation
│   ├── transpiler/        # Rust + Go + TypeScript transpilers
│   ├── manifest.rs        # mumei.toml parsing ([package]/[build]/[dependencies]/[proof])
│   ├── registry.rs        # Local package registry (~/.mumei/registry.json)
│   ├── setup.rs           # Toolchain installer (Z3 + LLVM download)
│   └── lsp.rs             # Language Server Protocol (hover, diagnostics)
├── std/
│   ├── prelude.mm         # Auto-imported: traits, ADTs, interfaces
│   ├── alloc.mm           # Vector<T>, HashMap<K,V>, ownership
│   ├── option.mm          # Option<T> + map_apply, and_then, filter
│   ├── result.mm          # Result<T,E> + map, and_then, wrap_err
│   ├── stack.mm           # Stack<T> + push/pop/clear
│   ├── list.mm            # List + immutable ops + sort + fold
│   └── container/
│       └── bounded_array.mm  # BoundedArray + sorted operations
├── editors/
│   └── vscode/            # VS Code extension (LSP client)
│       ├── package.json
│       ├── src/extension.ts
│       └── language-configuration.json
├── .github/
│   └── workflows/
│       └── release.yml    # Cross-platform binary release (macOS/Linux)
├── examples/              # call_test, match_atm, match_evaluator, import_test, higher_order_demo
├── tests/
│   ├── test_std_import.mm
│   ├── test_forall_ensures.mm
│   └── negative/          # 9 negative test files
├── docs/
│   ├── LANGUAGE.md        # Language reference
│   ├── STDLIB.md          # Standard library reference
│   ├── EXAMPLES.md        # Examples & test suite reference
│   ├── ARCHITECTURE.md    # Compiler internals
│   ├── TOOLCHAIN.md       # Toolchain & package management
│   └── CHANGELOG.md       # Change history
├── build_and_run.sh       # Build + test runner
└── Cargo.toml
```

---

## 🗺️ Roadmap

- [x] Refinement Types (Z3-backed)
- [x] `while` + loop invariant verification
- [x] Termination checking (`decreases` clause with ranking function)
- [x] Structs with per-field `where` constraints
- [x] Struct field access (`v.x`) and struct init (`Name { field: expr }`)
- [x] `f64` literals / `u64` base type support
- [x] Standard library function calls (`sqrt`, `len`, `cast_to_int`)
- [x] Float arithmetic sign propagation (pos×pos→pos, pos+non-neg→pos, etc.)
- [x] Per-array length model with symbolic bounds checking
- [x] Structured error types (`MumeiError` enum)
- [x] `VCtx` context object for verification (reduced function signatures)
- [x] `llvm!` macro for codegen boilerplate reduction
- [x] Comprehensive verification suite (8 atoms: stack ops, geometry, termination)
- [x] Module system (`import "path" as alias;` with recursive resolution)
- [x] Circular import detection
- [x] Inter-atom function calls with contract-based verification (compositional verification)
- [x] LLVM IR `declare` + `call` for user-defined atom calls
- [x] `ModuleEnv` architecture: zero global state, all definitions via struct (no Mutex)
- [x] Verification cache (`.mumei_cache`) with SHA-256 hash-based invalidation
- [x] Imported atom body re-verification skip (contract-trusted)
- [x] Transpiler module headers (`mod`/`use` for Rust, `package`/`import` for Go, `import` for TypeScript)
- [x] Enum (ADT) definitions (`enum Shape { Circle(f64), Rect(f64, f64), None }`)
- [x] Pattern matching (`match expr { Pattern => expr, ... }`)
- [x] Z3-powered exhaustiveness checking (SMT-based, not syntactic)
- [x] Match guard conditions (`Pattern if cond => ...`)
- [x] Default arm optimization (prior arm negations as preconditions for `_` arms)
- [x] Nested pattern decomposition (recursive `Variant(Variant(...))` support)
- [x] Counter-example display on exhaustiveness failure (Z3 `get_model()`)
- [x] Pattern Matrix codegen: linear if-else chain with clean CFG (no post-hoc switch insertion)
- [x] Recursive ADT support in parser (`Self` / self-referencing Enum fields)
- [x] Z3 Enum domain constraints: `0 <= tag < n_variants` auto-injected for Variant patterns
- [x] Projector-based field binding: `__proj_{Variant}_{i}` symbols shared across match arms
- [x] Recursive ADT bounded verification: recursive fields get domain constraints automatically
- [x] Enhanced counter-example display: Enum variant name + field types on exhaustiveness failure
- [x] Transpiler: Enum definitions → Rust enum / Go const+type / TypeScript const enum + discriminated union
- [x] Transpiler: Struct definitions → Rust struct / Go struct / TypeScript interface
- [x] Verified standard library: `std/option.mm`, `std/stack.mm`, `std/result.mm`, `std/list.mm`
- [x] **Std path resolution**: `import "std/option"` auto-resolves via project root / compiler dir / `MUMEI_STD_PATH`
- [x] **Generics (Polymorphism)**: `struct Pair<T, U>`, `enum Option<T>`, `atom identity<T>(x: T)` with monomorphization
- [x] **TypeRef**: Nested generic type references (`Map<String, List<i64>>`) with `substitute()` for type variable replacement
- [x] **Monomorphizer**: Collects generic instances from usage sites, expands to concrete definitions
- [x] **Trait system with Laws**: `trait Comparable { fn leq(...); law reflexive: ...; }` — algebraic laws as Z3 axioms
- [x] **Trait bounds**: `atom min<T: Comparable>(a: T, b: T)` — type constraints with `+` for multiple bounds
- [x] **impl verification**: Z3 verifies that `impl` satisfies all trait laws (method completeness + law satisfaction)
- [x] **Built-in traits**: `Eq` (reflexive, symmetric), `Ord` (reflexive, transitive), `Numeric` (commutative_add) — auto-implemented for i64/u64/f64
- [x] **Transpiler: Trait/Impl**: Rust `trait`/`impl` / Go `interface`/methods / TypeScript `interface`/const objects
- [x] **codegen ModuleEnv**: LLVM IR codegen uses `ModuleEnv` for all type/atom/struct/enum resolution
- [x] **CLI subcommands**: `mumei build` / `mumei verify` / `mumei check` / `mumei init`
- [x] **Project scaffolding**: `mumei init my_project` generates `mumei.toml` + `src/main.mm`
- [x] **Backward compatibility**: `mumei input.mm -o dist/katana` works as `mumei build`
- [x] **`std/prelude.mm`**: Auto-imported standard prelude — `Eq`, `Ord`, `Numeric` traits (with Z3 laws), `Option<T>`, `Result<T, E>`, `List<T>`, `Pair<T, U>` ADTs, `Sequential`/`Hashable` abstract interfaces
- [x] **Trait method refinement constraints**: `fn div(a: Self, b: Self where v != 0) -> Self;` — per-parameter `where` clauses on trait methods, parsed and stored as `param_constraints`
- [x] **Law body expansion (verify_impl)**: `substitute_method_calls()` expands law expressions by replacing method calls with impl bodies (e.g., `add(a,b)` → `(a + b)`), enabling precise Z3 verification with word-boundary-aware substitution
- [x] **alloc roadmap design**: `Vector<T>` / `HashMap<K, V>` architecture documented in `std/prelude.mm` with `Sequential`/`Hashable` trait interfaces as migration bridge
- [x] **Dynamic memory foundation**: `RawPtr`/`NullablePtr` refined types, `Owned` trait (linearity law), `Vector<T>` struct with `ptr`/`len`/`cap` field constraints, verified `vec_push`/`vec_get`/`vec_drop`/`vec_push_safe` atoms
- [x] **Linearity checking (LinearityCtx)**: Ownership tracking context for double-free and use-after-free detection — `register()`, `consume()`, `check_alive()` with violation accumulation
- [x] **`consume` parameter modifier**: `atom take(x: T) consume x;` — parsed via `consumed_params`, integrated with `LinearityCtx` + Z3 `__alive_` symbolic Bools for compile-time double-free/use-after-free detection
- [x] **LLVM alloc/dealloc codegen**: `alloc_raw` → `malloc` (with `ptr_to_int`), `dealloc_raw` → `free` (with `int_to_ptr`) — native heap operations in LLVM IR
- [x] **Borrowing (`ref` keyword)**: `atom print(ref v: Vector<i64>)` — `Param.is_ref` flag parsed, `LinearityCtx.borrow()`/`release_borrow()` for lifetime tracking, Z3 `__borrowed_` symbolic Bools prevent consume during borrow
- [x] **Transpiler ownership mapping**: Rust: `ref` → `&T`, `consume` → move semantics; TypeScript: `ref` → `/* readonly */` annotation; Go: comment-based ownership documentation
- [x] **`HashMap<K, V>`**: `struct HashMap<K, V> { buckets, size, capacity }` with field constraints, verified `map_insert`/`map_get`/`map_contains_key`/`map_remove`/`map_rehash`/`map_insert_safe`/`map_should_rehash` atoms in `std/alloc.mm`
- [x] **Equality ensures propagation**: `ensures: result == n + 1` now propagates through chained calls — `propagate_equality_from_ensures()` recursively extracts `result == expr` from compound ensures (`&&`-joined) and asserts Z3 equality constraints
- [x] **Negative test suite design**: Test categories documented — postcondition violation, division-by-zero, array out-of-bounds, match exhaustiveness, ownership double-free, use-after-free, ref+consume conflict (test files to be created in `tests/negative/`)
- [x] **Struct method definitions**: `StructDef.method_names` field added — supports `impl Stack { atom push(...) }` pattern with FQN registration as `Stack::push` in ModuleEnv
- [x] **FQN dot-notation**: `math.add(x, y)` resolved as `math::add` in both verification (`expr_to_z3`) and codegen (`compile_expr`) — `.` → `::` automatic conversion
- [x] **Incremental build**: `.mumei_build_cache` with per-atom SHA-256 hashing (`compute_atom_hash`) — unchanged atoms skip Z3 verification in both `mumei verify` and `mumei build`, with cache invalidation on failure
- [x] **Nested struct support**: `v.point.x` resolved via recursive `build_field_path()` → `["v", "point", "x"]` → env lookup as `v_point_x` / `__struct_v_point_x`, with recursive `extract_value` in LLVM codegen
- [x] **Async/Await + Resource Hierarchy**: `async atom`, `acquire r { body }`, `await expr` — Z3 resource priority ordering, await-across-lock detection, ownership consistency at suspension points
- [x] **Mutable References (`ref mut`)**: `atom modify(ref mut v: i64)` — Z3 exclusivity constraint (`__exclusive_`), aliasing prevention (same-type `ref`+`ref mut` forbidden unless provably distinct)
- [x] **Trust Boundary**: `trusted atom` (body skip) / `unverified atom` (warning) — FFI safety with taint analysis (`__tainted_` markers)
- [x] **BMC (Bounded Model Checking)**: Loop-internal `acquire` patterns unrolled up to `max_unroll: N;` (default: 3) — Z3 timeout guard
- [x] **Inductive Invariant**: `invariant: expr;` on atoms — base case + preservation proof, upgrades BMC to complete proof
- [x] **Call Graph Cycle Detection**: DFS-based indirect recursion detection (A→B→A) with `invariant`/`max_unroll` guidance
- [x] **Taint Analysis**: `unverified` function return values marked `__tainted_`, warning on use in safety proofs
- [x] **Pre-commit hooks**: `check-yaml` + `cargo fmt` + `cargo clippy` + `cargo test` via `.pre-commit-config.yaml`
- [x] **Verified standard library (enhanced)**: Option/Result map/andThen/filter, List immutable ops + fold, sort algorithms, BoundedArray
- [x] **`forall`/`exists` in ensures**: Quantifiers in postconditions via `expr_to_z3` Call handler
- [x] **`mumei inspect`**: Environment inspection command (Z3, LLVM, Rust, Go, Node.js, std library)
- [x] **`mumei.toml` parsing**: `manifest.rs` reads `[package]`, `[build]`, `[dependencies]`, `[proof]` — `cmd_build` auto-applies `targets`, `verify`, `max_unroll`, `timeout_ms`, `cache`
- [x] **Dependency resolution**: `mumei add` writes path/git deps to `mumei.toml`; `resolver::resolve_manifest_dependencies()` auto-fetches path deps and `git clone`s git deps to `~/.mumei/packages/`
- [x] **Toolchain setup (`mumei setup`)**: Downloads Z3 + LLVM pre-built binaries to `~/.mumei/toolchains/`, generates `~/.mumei/env` script
- [x] **LSP server (`mumei lsp`)**: JSON-RPC stdio server with `textDocument/hover` (atom contract display), `publishDiagnostics` (parse errors + Z3 verification errors)
- [x] **VS Code Extension**: `editors/vscode/` — LSP client package, language configuration for `.mm` files
- [x] **GitHub Actions Release**: `.github/workflows/release.yml` — cross-platform binary builds (macOS x86_64/aarch64, Linux x86_64) with std library bundled
- [x] Higher-order functions Phase A: `atom_ref` + `call` — direct `atom_ref(name)` contract propagation works; parametric `f: atom_ref(T) -> R` parameters require `trusted` (Phase B will add `call_with_contract`)
- [x] **`mumei publish`**: Local registry (`~/.mumei/packages/`) publishing with proof caching — `mumei publish` verifies all atoms, copies to `~/.mumei/packages/<name>/<version>/`, registers in `~/.mumei/registry.json`; `--proof-only` flag for cache-only publish
- [x] **Registry-based dependency resolution**: `math = "0.1.0"` in `mumei.toml` resolves via `~/.mumei/registry.json` → `~/.mumei/packages/` (priority: path → registry → git)
- [ ] Remote package registry: Central registry for `mumei add <name>` without git URL
- [ ] VS Code Marketplace publishing: Package and publish `editors/vscode/` as installable extension
- [ ] LSP enhancements: `textDocument/completion` (keyword/atom name), `textDocument/definition` (jump to definition), counter-example highlighting
- [x] **Diagnostics-Driven Design**: `Span` (file/line/col/len) attached to all AST nodes, `ErrorDetail` + `MumeiError` with Span integration, LSP diagnostics with source positions → [`docs/DIAGNOSTICS.md`](docs/DIAGNOSTICS.md)
- [x] **Rich Diagnostics (miette)**: Colored terminal output with source code highlighting, underline pointers, and actionable help suggestions via [miette](https://crates.io/crates/miette) — `MumeiError` derives `thiserror::Error` + `miette::Diagnostic`, `span_to_source_span()` for byte-offset conversion, `.with_source()` / `.with_help()` builder methods
- [x] **FFI extern syntax**: `extern "Rust" { fn sqrt(x: f64) -> f64; }` parsing with `ExternFn`/`ExternBlock` AST + Span → [`docs/FFI.md`](docs/FFI.md)
- [ ] **FFI Bridge (auto-registration)**: Auto-convert extern declarations to `trusted atom`, direct Rust crate references (not yet implemented)
- [x] **Structured Concurrency (parser + Z3)**: `task { ... }` / `task_group { ... }` / `task_group:all` / `task_group:any` syntax, symbolic Z3 join constraints, full AST walker support, multi-language transpilation → [`docs/CONCURRENCY.md`](docs/CONCURRENCY.md)
- [ ] **Structured Concurrency (runtime)**: Runtime scheduler, task cancellation, channel types (not yet implemented)
- [ ] **`std.http`**: HTTP client wrapping Rust `reqwest` via FFI, parallel request integration with `task_group`
- [ ] **`std.json`**: String/object conversion with serde_json backend, type-safe JSON operations
- [ ] **Task refinements**: Return type inference, `task_group` result binding, channel types (`chan<T>`)
- [ ] **Runtime Portability**: musl static linking, Homebrew Tap, WebInstall (`curl | sh`)
- [ ] **`mumei repl`**: Interactive execution environment (parse → verify → eval loop)
- [ ] **`mumei doc`**: Auto-generate HTML documentation from source comments (rustdoc-style)

> 📖 **Strategic roadmap**: [`docs/ROADMAP.md`](docs/ROADMAP.md) | **Toolchain**: [`docs/TOOLCHAIN.md`](docs/TOOLCHAIN.md) | **Development instructions**: [`instruction.md`](instruction.md)
