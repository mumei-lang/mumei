# 🗺️ Strategic Roadmap — Mumei v0.3.0+

> Three strategic roadmap priorities to evolve Mumei from an experimental language to a practical tool.

## Overview

| Priority | Theme | Goal | Status |
|---|---|---|---|
| 🥇 P1 | Network-First Standard Library | Practical utility as an API scripting language | ✅ Implemented |
| 🥈 P2 | Runtime Portability | Run-anywhere distribution foundation | ✅ Implemented |
| 🥉 P3 | CLI Developer Experience | World-class CLI developer experience | ✅ Implemented |

---

## 🥇 Priority 1: Network-First Standard Library

### Vision

HTTP requests and JSON operations should be "standard equipment" in modern programming.
Leveraging the FFI foundation from PR #29, we prioritize **wrapping Rust's power in Mumei's skin**.

**Goal**: Create motivation to write "scripts that hit APIs and process data" in Mumei.

### Phase A: FFI Bridge Completion

Complete auto-conversion from extern declarations to trusted atoms.
This is the **prerequisite** for std.http / std.json.

**Current State**:
- ✅ `extern "Rust" { fn sqrt(x: f64) -> f64; }` syntax parsed
- ✅ `ExternFn` / `ExternBlock` AST + Span
- ✅ `Item::ExternBlock` all match arms covered
- ✅ extern → ModuleEnv auto-registration (trusted atom) — implemented in PR #32
- ✅ LLVM codegen (extern function declare + call)

**Implementation Plan**:

```
1. ExternBlock → trusted atom auto-conversion
   - Generate Atom from ExternFn signature
   - Set TrustLevel::Trusted (skip body verification)
   - Auto-register in ModuleEnv.atoms

2. LLVM declare generation
   - Output extern functions as LLVM IR `declare`
   - Type mapping: Mumei types → LLVM types

3. Call-site code generation
   - Generate call to extern atoms registered in ModuleEnv
   - Ensure ABI compatibility (extern "C" / extern "Rust")
```

**Files to modify**:
- `src/main.rs` — ExternBlock → atom conversion in `load_and_prepare()`
- `mumei-core/src/verification.rs` — trusted verification for extern atoms
- `mumei-emit-llvm/src/codegen.rs` — LLVM `declare` + `call` generation
- `docs/FFI.md` — implementation status update

### Phase B: std.json

String/object conversion. Combine with Mumei's type inference for type-safe JSON handling.

**Target API**:

```mumei
import "std/json" as json;

// Parse: string → structured data
let data = json.parse(raw_string);

// Stringify: structured data → string
let output = json.stringify(data);

// Type-safe field access
let name = json.get_string(data, "name");
let age = json.get_int(data, "age");
```

**Backend**: `serde_json` (already a Cargo.toml dependency)

**Files to create/modify**:
- `std/json.mm` — JSON operation atom definitions
- `mumei-core/src/parser/` — string literal type extension (if needed)
- `docs/STDLIB.md` — std.json reference

### Phase C: std.http (Client)

HTTP client wrapping `reqwest` behind FFI backend.

**Target API**:

```mumei
import "std/http" as http;

// Simple GET — maximum simplicity
let response = await http.get("https://api.example.com/users");
let status = http.status(response);
let body = http.body(response);

// POST with JSON body
let response = await http.post("https://api.example.com/users", payload);
```

**Backend**: Rust `reqwest` crate (via FFI)

**Files to create/modify**:
- `std/http.mm` — HTTP operation atom definitions
- `Cargo.toml` — `reqwest` dependency
- `docs/STDLIB.md` — std.http reference

### Phase D: Integration Demo

Integration demo with `task_group` for parallel requests.

```mumei
import "std/http" as http;
import "std/json" as json;

// Concurrent API requests — Mumei's killer feature
task_group:all {
    task { http.get("https://api.example.com/users") };
    task { http.get("https://api.example.com/orders") };
    task { http.get("https://api.example.com/products") }
}
```

**Files to create**:
- `examples/http_demo.mm` — HTTP demo
- `examples/json_demo.mm` — JSON demo
- `examples/concurrent_http.mm` — Parallel HTTP demo

---

## 🥈 Priority 2: Runtime Portability

### Vision

"Running anywhere" is an absolute requirement for adoption.
Reduce the installation barrier to near-zero and target the niche of
"quick automation scripts" in GitHub Actions and CI/CD environments.

### Phase A: Static Linking Optimization

Statically link all shared library dependencies so that a single `mumei`
executable runs anywhere.

**Current State**:
- ✅ GitHub Actions release workflow (macOS x86_64/aarch64, Linux x86_64)
- ✅ `mumei setup` for Z3/LLVM auto-download
- ✅ musl target (fully static linking) — added in Plan 7
- ✅ Windows binaries (`x86_64-pc-windows-msvc`) — added in Plan 7

**Implementation Plan**:

```
1. Add musl target
   - x86_64-unknown-linux-musl target
   - Add musl build job to GitHub Actions

2. Verify static linking of dependencies
   - Z3: verify static linking feasibility
   - LLVM: confirm static link settings
   - Verify with ldd on all targets

3. Windows support (stretch goal)
   - x86_64-pc-windows-msvc target
   - Add Windows job to GitHub Actions
```

**Files to modify**:
- `.github/workflows/release.yml` — add musl/Windows builds
- `Cargo.toml` — static link settings
- `docs/TOOLCHAIN.md` — update supported platforms

### Phase B: Homebrew Tap

One-command installation via `brew install mumei-lang/mumei`.

**Implementation Plan**:

```
1. ✅ Create mumei-lang/homebrew-mumei repository
2. ✅ Create Formula (download from GitHub Releases)
3. ✅ Auto-update Formula via CI (release.yml integration)
   — Formula テンプレートは scripts/generate_formula.py に分離し、
     update-homebrew ジョブから呼び出してローカルでも再現可能。
```

**Formula example**:
```ruby
class Mumei < Formula
  desc "Mathematical Proof-Driven Programming Language"
  homepage "https://github.com/mumei-lang/mumei"
  url "https://github.com/mumei-lang/mumei/releases/download/v0.3.0/mumei-aarch64-apple-darwin.tar.gz"
  sha256 "..."
  license "MIT"

  def install
    bin.install "mumei"
    (share/"mumei-std").install Dir["std/*"]
  end
end
```

### Phase C: WebInstall (curl | sh)

```bash
curl -fsSL https://mumei-lang.github.io/install.sh | sh
```

**Implementation Plan**:

```
1. Create install.sh script
   - Auto-detect OS/arch
   - Download latest binary from GitHub Releases
   - Guide user to add to PATH

2. Host on GitHub Pages
3. Add installation instructions to README
```

**Files to create**:
- `scripts/install.sh` — installer script
- `.github/workflows/release.yml` — auto-update install.sh

---

## 🥉 Priority 3: CLI Developer Experience

### Vision

Instead of focusing on LSP, we aim for world-class "CLI-based development experience".
Languages with great documentation enable users to be self-sufficient,
and communities grow organically.

### Phase A: mumei repl

Enhanced REPL (Read-Eval-Print Loop) for experimenting with syntax
and trying HTTP requests.

**Target UX**:

```
$ mumei repl
Mumei v0.3.0 REPL — type :help for commands, :quit to exit

mumei> type Nat = i64 where v >= 0;
Type defined: Nat

mumei> atom inc(n: Nat) requires: n >= 0; ensures: result >= 1; body: n + 1;
✅ Verified: inc

mumei> inc(5)
= 6

mumei> inc(-1)
❌ Verification failed: requires n >= 0, but got n = -1

mumei> :load examples/http_demo.mm
Loaded 3 atoms from examples/http_demo.mm

mumei> :quit
```

**Implementation Plan**:

```
1. REPL loop foundation
   - rustyline (line editing + history) or stdin-based
   - parse → verify → eval pipeline

2. Incremental definitions
   - Append to ModuleEnv incrementally
   - Support definition overwriting

3. Special commands
   - :help, :quit, :load, :env (list current definitions)
   - :type <expr> (display type inference result)

4. HTTP/JSON integration (after P1 completion)
   - Execute http.get() directly from REPL
```

**Files to create/modify**:
- `src/repl.rs` — REPL engine
- `src/main.rs` — `mumei repl` subcommand
- `Cargo.toml` — `rustyline` dependency

### Phase B: mumei doc

Generate beautiful HTML documentation from source code comments,
similar to Rust's `rustdoc`.

**Target UX**:

```bash
$ mumei doc src/main.mm -o docs/

# Generates:
# docs/index.html
# docs/atoms/increment.html   (requires/ensures/body)
# docs/types/Nat.html          (refinement predicate)
# docs/traits/Comparable.html  (methods + laws)
```

**Doc comment syntax**:

```mumei
/// Increments a natural number by 1.
///
/// # Examples
/// ```
/// inc(5) == 6
/// inc(0) == 1
/// ```
atom inc(n: Nat)
    requires: n >= 0;
    ensures: result >= 1;
    body: n + 1;
```

**Implementation Plan**:

```
1. Doc comment parser
   - Extract /// comments
   - Markdown parsing (lightweight)

2. HTML template engine
   - Pages for atom / type / trait / struct / enum
   - Index page (all definitions)
   - requires/ensures visualization

3. CSS styling
   - Dark mode support
   - Syntax highlighting

4. CLI integration
   - mumei doc <input> -o <output_dir>
   - mumei doc --json (structured output)
```

**Files to create/modify**:
- `src/doc.rs` — documentation generation engine
- `src/main.rs` — `mumei doc` subcommand
- `templates/` — HTML templates

### Phase C: REPL + HTTP Integration

Demo for trying HTTP requests directly from REPL (after P1 + P3A completion).

```
mumei> import "std/http" as http;
mumei> let res = await http.get("https://httpbin.org/get");
mumei> http.status(res)
= 200
mumei> http.body(res)
= "{ \"origin\": \"...\" }"
```

---

## Dependencies

```
P1-A (FFI Bridge) ──→ P1-B (std.json) ──→ P1-D (Integration Demo)
                  ──→ P1-C (std.http)  ──→ P1-D
                                        ──→ P3-C (REPL + HTTP)

P2-A (Static Link) ──→ P2-B (Homebrew) ──→ P2-C (WebInstall)

P3-A (REPL) ─────────→ P3-C (REPL + HTTP)
P3-B (mumei doc)       (independent)
```

---

## Success Metrics

| Metric | Target | Measurement |
|---|---|---|
| **API Script Demo** | `http.get` + `json.parse` working | examples/http_demo.mm passes |
| **Install Time** | < 30 seconds | `curl \| sh` from clean environment |
| **REPL Responsiveness** | < 100ms per eval | Benchmark on standard hardware |
| **Doc Coverage** | 100% of std library | `mumei doc std/` generates all pages |
| **Binary Size** | < 50MB (static) | `ls -la target/release/mumei` |
| **Platform Support** | macOS + Linux + Windows | CI green on all targets |

---

## Timeline (Estimated)

| Phase | Duration | Milestone |
|---|---|---|
| P1-A: FFI Bridge | 1-2 weeks | extern → trusted atom auto-registration |
| P1-B: std.json | 1 week | `json.parse` / `json.stringify` |
| P1-C: std.http | 1-2 weeks | `http.get` / `http.post` |
| P1-D: Demo | 1 week | Integration demo + documentation |
| P2-A: Static Link | 1 week | musl build + CI |
| P2-B: Homebrew | 1 week | `brew install mumei` |
| P2-C: WebInstall | 1 week | `curl \| sh` |
| P3-A: REPL | 2 weeks | `mumei repl` basic functionality |
| P3-B: Doc Gen | 2-3 weeks | `mumei doc` HTML generation |
| P3-C: Integration | 1 week | REPL + HTTP integration |

---

## P4: Effect System — Inference, Refinement, Hierarchy

### Current Implementation

- **Effect Inference**: Call graph traversal infers required effects from callee atoms
- **Hybrid Path Verification**: Constant Folding (Rust-side) + Z3 String Sort (symbolic paths)
- **Effect Hierarchy (Subtyping)**: `parent:` field on EffectDef enables Network → HttpRead/TcpConnect
- **MCP Pre-check**: `get_inferred_effects` tool lets AI check required permissions before writing code

### Z3 String Sort Integration (Complete)

Z3's native String sort has been integrated for symbolic effect parameter verification.
The hybrid approach (Constant Folding + Z3 String Sort) is now active:

**Completed**:
- ✅ `z3` crate 0.12.1 confirmed with stable `z3::ast::String` support
- ✅ `z3::ast::String` imported as `Z3String` in verification.rs
- ✅ `parse_constraint_to_z3_string()` maps constraint strings to Z3 String operations:
  - `starts_with(path, "/tmp/")` → `Z3String::prefix_of`
  - `ends_with(path, ".txt")` → `Z3String::suffix_of`
  - `contains(path, "data")` → `Z3String::contains`
  - `not_contains(path, "..")` → `NOT Z3String::contains`
- ✅ Perform handler extended: symbolic (variable) args verified via Z3 String constraints
- ✅ Sort-aware timeout: Z3 solving timeout doubled when String constraints are present
- ✅ Constraint budget checked on String constraint creation
- ✅ Performance validated: 10 String constraints solve in < 500ms

**Hybrid Strategy**:
- Constant paths: verified by `check_constant_constraint()` (Rust-side, zero Z3 overhead)
- Symbolic paths (variables): verified by Z3 String Sort constraints in the solver
- `path_id_map` / `prefix_ranges` retained as `#[allow(dead_code)]` for future Int encoding fallback

**Future Unlocks**:
- ~~Free-form path construction: `"/tmp/" + user_id + "/log.txt"` verification~~ ✅ Implemented (Plan 21)
- ~~Regex-based path policies: `matches(path, "/tmp/[a-z]+\\.txt")`~~ ✅ Implemented (Plan 23)
- ~~URL validation for std.http effects: `starts_with(url, "https://")`~~ ✅ Implemented (Plan 23)

### Effect Hierarchy Extensions

Extensions to the effect subtyping system:

1. **Multi-parent (Intersection)**: `effect SecureNetRead parent: [Network, Encrypted];` — ✅ Done (Plan 6)
2. **Effect polymorphism**: `atom pipe<E: Effect>(f: atom_ref(T) -> U with E)` — ✅ Done
3. **Effect narrowing**: When calling a `Network`-annotated function with only `HttpRead`, narrow the effect at the call site — ✅ Done (Plan 6, info diagnostic)
4. **Negative effects**: `atom pure_compute() effects: [!IO];` — explicitly deny effects — ✅ Done (Plan 6)
5. **Effect aliases**: `effect IO = FileRead | FileWrite | ConsoleOut;` — union types for convenience — ✅ Done (Plan 6)

---

## Multi-Stage IR Roadmap

| Phase | Item | Status | Prerequisite |
|---|---|---|---|
| Phase 0 | Expr/Stmt separation | ✅ Done | — |
| Phase 1 | HIR introduction (typed AST, eliminate String-based body_expr) | ✅ Done | Phase 0 |
| Phase 2 | Basic Effect System (parameterized effects, security policy) | ✅ Done | ✅ Expression parser migrated to recursive descent (item parsing still regex) |
| Phase 2.5 | Lambda / Closure Support | ✅ Done | Phase 2 |
| Phase 2.5 | Semantic Feedback v2 (all failure types, bilingual) | ✅ Done | Phase 1 |
| Phase 3 | Effect Polymorphism | ✅ Done | Phase 2 |
| Phase 4 | MIR introduction (CFG for borrow checking) | ✅ Phase 4a-4c done: liveness, move analysis, Copy/Move distinction, drop insertion | LinearityCtx wired + MIR data structures + mir_analysis.rs |
| Phase 5 | HIR Effect Type Information | ✅ Done | HirEffectSet on HirAtom/HirExpr, lower_atom_to_hir_with_env |
| Phase 6 | Capability Security evaluation | ✅ Done | See docs/CAPABILITY_SECURITY.md |
| Phase 7 | Temporal Effect Verification (Stateful Effects) | ✅ Done | EffectStateMachine, forward dataflow, Phase 1i |
| Phase 8 | Modular Verification (effect_pre / effect_post) | ✅ Done | Cross-atom temporal effect state tracking via contracts |

### Why Phases 2–5 Are Deferred

- **Phase 2 (Basic Effects)**: ✅ Complete — parameterized effects (`FileRead(path: Str)`, `HttpGet(url: Str)`) implemented with security policy enforcement. Standard library effects defined in `std/effects.mm`, `std/http.mm`, `std/file.mm`. Z3 verifies parameter constraints (e.g., `starts_with(path, "/tmp/")`) at compile time.
- **Phase 3 (Effect Polymorphism)**: ✅ Complete — Effect polymorphism via `<E: Effect>` bounds and `with E` syntax. Resolved through monomorphization (same as type polymorphism).
- **Phase 4 (MIR)**: A CFG-based intermediate representation is needed for borrow checking and lifetime analysis, but the borrow checking design itself is not yet started. Will be introduced after the design is finalized.
- **Phase 5 (HIR Effect Type Information)**: ✅ Complete — `HirEffectSet` attached to `HirAtom`, `HirExpr::Call`, `HirExpr::Perform`. `lower_atom_to_hir_with_env()` populates effect info from `ModuleEnv`. Codegen reads effects from `hir_atom.effect_set`.
- **Phase 6 (Capability Security)**: ✅ Complete — Evaluation documented in `docs/CAPABILITY_SECURITY.md`. Recommendation: Continue with parameterized effects + Z3 (Option A). `EffectCtx`, `SecurityPolicy`, `verify_effect_params`, `verify_effect_consistency`, `build_effect_feedback` all wired into the verification pipeline.

---

## Phase 4c+ Implementation Plans

Detailed session plans for the next 8 implementation priorities are documented in [SESSION_PLANS.md](./SESSION_PLANS.md).

| # | Plan | Status |
|---|------|--------|
| 1 | Phase 4c: MIR Copy/Move type distinction | ✅ Implemented |
| 2 | MIR Lowering: remaining expression forms | ✅ Implemented |
| 3 | MIR control flow lowering hardening | ✅ Implemented |
| 4 | MIR Drop Insertion: SwitchInt successor drops | ✅ Implemented |
| 5 | Z3 String Sort migration | ✅ Implemented |
| 6 | Effect Hierarchy extensions | ✅ Implemented |
| 7 | Runtime Portability: musl + Windows | ✅ CI infrastructure verified and stable |
| 8 | Concurrency improvements | ✅ Parser/AST/HIR infrastructure added (codegen placeholder) |
| 9 | Plan 15: Examples + E2E tests | ✅ 5 examples + 3 test files |
| 10 | Plan 16: FFI memory management | ✅ json_free/string_free/http_free |
| 11 | Plan 17: Str type migration | ✅ Examples use Str-typed parameters |
| 12 | Plan 18: Codegen return types | ✅ `resolve_return_type()`, `-> Type` syntax |
| 13 | Plan 19: MIR Phase 4c completion | ✅ MoveAnalysis is primary engine |
| 14 | Plan 20: Z3 temporal effect integration | ✅ `encode_effect_state()`, ConflictingState Z3 probes |
| 15 | Plan 21: Verified HTTP Server + Path Safety | ✅ SafeFileRead/SafeFileWrite effects, `&&` compound constraints, HTTP server FFI, HttpServer stateful effect, path traversal prevention demo |
| 16 | Plan 22: PII Pipeline Example | ✅ DataPipeline temporal effect demo + E2E tests |
| 17 | Plan 23: Regex Path Policies + URL Validation | ✅ RegexSafeFileRead, SecureHttpGet/Post, Z3 approximation improvements |
| 18 | Plan 24: Modular Verification | ✅ effect_pre/effect_post contracts, cross-atom temporal state tracking |
| 19 | Plan 25: LSP Completion & Definition | ✅ textDocument/completion, textDocument/definition, multi-editor docs |

### Plan 22: PII Pipeline Example

A practical demonstration of Temporal Effect Verification applied to data privacy enforcement.
The `DataPipeline` stateful effect defines `Raw` and `Anonymized` states with transitions
`load: Raw → Raw`, `anonymize: Raw → Anonymized`, and `log: Anonymized → Anonymized`.
This ensures that personal data **must** pass through anonymization before it can be logged —
any attempt to log raw data is caught at compile time as an `InvalidPreState` violation.

**Files**:
- `examples/pii_pipeline.mm` — Valid pipeline demonstrating correct load → anonymize → log sequence
- `examples/pii_pipeline_error.mm` — Invalid pipeline (skips anonymize) showing compile-time rejection
- `tests/test_pii_pipeline.mm` — E2E integration test with multiple valid pipeline patterns
- `src/mir_analysis.rs` — 3 unit tests: valid sequence, skip anonymize (InvalidPreState), branch conflict (ConflictingState)

### Plan 23: Regex Path Policies + URL Validation

Extends the P4 effect system with regex-based path constraints and HTTPS URL validation.

**Regex Path Policies**:
- `RegexSafeFileRead(path: Str) where matches(path, "^/tmp/[a-z]+/.*")` in `std/effects.mm`
- Z3 approximation improvements: exact match (`^literal$`) and prefix+suffix (`^prefix.*suffix$`) patterns

**URL Validation**:
- `SecureHttpGet(url: Str) where starts_with(url, "https://")` in `std/http.mm`
- `SecureHttpPost(url: Str) where starts_with(url, "https://")` in `std/http.mm`
- Backward compatible: existing `HttpGet`/`HttpPost` unchanged

**Files**:
- `std/effects.mm` — Added `RegexSafeFileRead` effect definition
- `std/http.mm` — Added `SecureHttpGet`/`SecureHttpPost` effect definitions
- `examples/regex_path_policy.mm` — Regex path constraint demo
- `examples/secure_http.mm` — HTTPS enforcement demo
- `tests/test_regex_policy.mm` — E2E test for regex path validation
- `tests/test_url_validation.mm` — E2E test for URL validation
- `src/verification.rs` — Z3 regex approximation improvements (exact match, prefix+suffix)

### Plan 24: Modular Verification (effect_pre / effect_post)

Adds cross-atom temporal effect state tracking via `effect_pre`/`effect_post` contracts.

**Syntax**:
```
atom open_file(x: i64)
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Open };
    ...
```

**Implementation**:
- `effect_pre` overrides initial state of corresponding state machines
- `effect_post` is checked against exit states; mismatch emits `UnexpectedFinalState`
- Invalid state names in `effect_pre`/`effect_post` produce hard errors; missing state machines emit warnings
- Monomorphizer substitutes effect type variables in keys (e.g., `{ E: Closed }` → `{ FileWrite: Closed }`)
- All Atom construction sites updated with default empty `HashMap`
- Parser extension for `{ Key: Value, Key2: Value2 }` syntax
- Cross-atom contract composition at call sites is now implemented via `analyze_temporal_effects_with_contracts()` (P2-A)

**Files**:
- `mumei-core/src/parser/ast.rs` — Added `effect_pre`/`effect_post` fields to `Atom` struct
- `mumei-core/src/parser/item.rs` — Parser for `effect_pre:`/`effect_post:` clauses
- `mumei-core/src/verification.rs` — Initial state override + final state check
- `src/main.rs`, `mumei-core/src/resolver.rs`, `mumei-core/src/ast.rs`, `mumei-core/src/mir.rs`, `mumei-core/src/mir_analysis.rs` — Updated Atom construction sites
- `tests/test_modular_verification.mm` — E2E test with File effect contracts
- `mumei-core/src/mir_analysis.rs` — 3 unit tests for modular verification
- `mumei-core/src/parser/mod.rs` — 3 parser tests for effect_pre/effect_post

### Plan 25: LSP Completion & Definition

Unfreezes the LSP server and adds two major features: textDocument/completion and textDocument/definition.

**textDocument/completion**:
- 56 mumei keywords returned as CompletionItem (kind=14 Keyword)
- Atom names extracted from parsed items cache (kind=3 Function)
- Effect names from EffectDef items (kind=8 Interface)
- Type/struct/enum names from TypeDef/StructDef/EnumDef items (kind=7 Class)
- Trigger characters: `.`, `:`

**textDocument/definition**:
- Extract word at cursor position from document text
- Search all cached parsed items for matching definitions (atom, type, struct, enum, effect, trait, resource)
- Return Location (URI + range) based on item's Span

**Performance: Parsed items cache**:
- `HashMap<String, Vec<Item>>` alongside existing `documents` HashMap
- Updated on every didOpen/didChange (reuses parse result from diagnose)
- Used for completion and definition lookups without re-parsing

**Multi-editor configuration docs**:
- `docs/EDITORS.md` with setup examples for Neovim, Helix, Emacs, Sublime Text, and Zed

**Files**:
- `src/lsp.rs` — Completion handler, definition handler, parsed items cache, keyword list, helper functions, unit tests
- `docs/EDITORS.md` — Editor configuration documentation (5 editors)
- `instruction.md` — §11 LSP status changed from "Frozen" to "Active"
- `docs/ROADMAP.md` — This plan entry

### P7: Runtime Completion (REPL JIT + Binary Execution)

Enables mumei's verified code to actually run — both interactively in the REPL and as standalone native binaries.

**P7-A: REPL Execution Engine (JIT)** — ✅ Implemented
- `mumei-emit-llvm/src/jit.rs` — JitEngine struct wrapping inkwell's ExecutionEngine (MCJIT)
- Refactored `codegen::compile()` into `compile_atom_into_module()` (in-memory) + `compile()` (file-based)
- `compile_to_module()` returns LLVM IR as string for standalone use
- REPL (`cmd_repl()`) enhanced with JIT: atom definitions are verified then JIT-compiled; expressions are wrapped as `__repl_eval` atoms, verified, executed, and results displayed
- `:eval <expr>` command for unverified JIT execution (debugging)
- `:load` now also compiles loaded atoms into the JIT module

**P7-B: End-to-End Binary Execution** — ✅ Implemented
- `EmitTarget::Binary` variant added to emitter
- `src/linker.rs` — finds clang and links LLVM IR to native binary (`clang -O2 -o <output> <merged.ll> -lm -lpthread`)
- `mumei-emit-llvm/src/binary.rs` — `compile_atoms_to_binary_ll()` merges all atoms into single LLVM module with C-compatible `main` wrapper
- `mumei run <file.mm>` CLI command: verify → compile → link → execute → cleanup
- FFI warning: extern blocks trigger a warning about runtime library requirement
- Examples: `examples/run_demo.mm`, `examples/run_with_calls.mm`

**Known Limitations**:
- **MCJIT incremental compilation**: The JIT engine uses MCJIT, which finalizes the entire module on first `get_function` call. Defining multiple interdependent atoms across REPL iterations and then calling them may fail. Single-eval usage (`:eval`, `:verify`, bare expressions) works correctly. A future migration to ORC JIT would resolve this.
- ~~**Binary compilation: top-level atoms only**: `mumei run` and `mumei build --emit binary` only compile top-level `atom` definitions. `impl` block methods are not included in the binary. Programs using struct methods will fail to link.~~ **Fixed**: `impl` block methods are now included in binary compilation with qualified names (`StructName::method_name`).
- ~~**Self-recursive `main` atom**: The rename strategy (`main` → `__mumei_user_main`) does not rename recursive calls inside the body. If `main` calls itself, the call target will reference the C wrapper instead.~~ **Fixed**: `rename_calls_in_hir_stmt/expr` now recursively renames all `main` calls to `__mumei_user_main` in the HIR tree.
- ~~**`find_clang()` is Unix-only**: Uses the `which` command, which is not available on Windows.~~ **Fixed**: `find_on_path()` helper uses `which` on Unix and `where` on Windows, with `clang.exe` fallback for Windows toolchain paths.

**Verification Domain Extension Patterns**:
- ✅ Verified Configuration Pattern (`examples/verified_config.mm`) — refinement types for configuration validation
- ✅ Verified State Machine Pattern (`examples/order_state_machine.mm`) — temporal effects for business process modeling
- See [`docs/PATTERNS.md`](PATTERNS.md) for detailed pattern documentation

**P7-C: Wasm Target** — Deferred
- WebAssembly compilation target for browser/edge execution
- Will be implemented after P7-A/B stabilize

**Future: Developer Experience** — Deferred
- Enhanced error messages, IDE integration improvements, debugging tools
- Will be implemented after runtime completion is stable

**Files**:
- `mumei-emit-llvm/src/jit.rs` — JIT execution engine (5 unit tests)
- `mumei-emit-llvm/src/binary.rs` — Binary compilation pipeline
- `mumei-emit-llvm/src/codegen.rs` — Refactored compile functions
- `mumei-emit-llvm/src/lib.rs` — Module exports + LlvmContext re-export
- `mumei-core/src/emitter.rs` — EmitTarget::Binary variant
- `src/linker.rs` — Clang linker pipeline
- `src/main.rs` — `cmd_run()`, REPL JIT enhancements, `Run` command variant
- `examples/run_demo.mm` — Simple binary execution demo
- `examples/run_with_calls.mm` — Multi-atom binary execution demo

---

### P8: 形式検証の理論的限界への対処

Z3 ベースの自動検証は Mumei の主要な強みだが、SMT ソルバのモデルは「仕様が正しい」ことや「反例が意味論的に正当である」ことまでは保証しない。
P8 では、形式仕様そのものの健全性を検査し、Z3 で扱える決定可能断片を明文化し、必要な場合だけ Lean 4 へエスカレーションする運用境界を定義する。

**P8-A: Spurious Counterexample Detection（偽反例検出）** — Planned

Lean 4 の `bv_decide` / BVDecide が反例を再構成して検証するアプローチを参照し、Z3 の `sat` モデルをそのまま信じず、Mumei の意味論で再評価するメタ検証層を追加する。

**Implementation Plan**:

```
1. 反例モデルの正当性チェック
   - Z3 から得たモデルを HIR/MIR 式へ再代入
   - requires / ensures / effect_pre / effect_post を Mumei 側で再評価
   - 再評価不能な項は "unvalidated_counterexample" として分類

2. Uninterpreted symbol detection
   - 未解釈関数・未展開 atom・trusted atom 由来のシンボルを抽出
   - 反例が未解釈シンボルの任意解釈だけに依存していないか検査
   - 証明失敗レポートに symbol provenance を付与

3. Unused hypothesis checking
   - unsat core / dependency trace から未使用 requires・invariant・effect 制約を検出
   - 未使用仮説が仕様の過剰拘束または死んだ仕様でないか警告
   - 反例の最小制約集合を proof certificate に保存
```

**Files to modify/create**:
- `mumei-core/src/verification.rs` — Z3 モデル取得、反例再代入、未解釈シンボル検出
- `mumei-core/src/proof_cert.rs` — 反例メタ検証結果、unused hypothesis、symbol provenance の証明書フィールド
- `src/main.rs` — CLI 診断出力に `validated_counterexample` / `spurious_candidate` を表示
- `tests/` — 偽反例・未解釈シンボル・未使用仮説の回帰テスト

**Success Metrics**:
- Z3 `sat` のうち Mumei 再評価に成功した反例率: ≥ 95%
- 未解釈シンボル依存の反例を `spurious_candidate` として分類できる率: ≥ 90%
- unused hypothesis 警告の false positive: < 5%

**P8-B: Specification Validation Framework（仕様検証フレームワーク）** — Planned

コードを証明する前に、仕様自体が矛盾・過剰拘束・自然言語プロンプトからの逸脱を含まないかを検証する。
特に AI 生成仕様では「実装は証明されるが、仕様が意図と違う」リスクを明示的に扱う。

**Implementation Plan**:

```
1. Contradiction detection for specs
   - requires の充足可能性を Z3 で事前チェック
   - ensures 同士、refinement type、effect state 制約の矛盾を検出
   - 矛盾仕様を proof attempt 前に SpecContradiction として停止

2. QuickCheck-style property-based testing
   - refinement type から入力ジェネレータを合成
   - ランダム・境界値・縮小 (shrinking) による仕様妥当性チェック
   - Z3 で unknown となる仕様にも実行的な sanity check を提供

3. Semantic traceability verification
   - 自然言語プロンプト、生成仕様、実装 atom の三者を trace_id で接続
   - prompt の must/never/only 条件が requires / ensures / effects に反映されたか検査
   - mumei-agent から受け取る forge task metadata と proof certificate を連携
```

**Files to modify/create**:
- `mumei-core/src/verification.rs` — 仕様充足可能性チェックと `SpecContradiction` 診断
- `mumei-core/src/parser/ast.rs` — optional `trace_id` / spec metadata の保持
- `mumei-core/src/proof_cert.rs` — spec validation 結果と traceability hash の記録
- `mcp_server.py` — AI 生成仕様の traceability metadata 入出力
- `docs/SPEC_GUIDE.md` — 仕様検証と property-based validation の利用ガイド

**Success Metrics**:
- 矛盾する requires を proof 前に検出する率: 100%
- property-based validation で発見された仕様欠陥の縮小反例出力率: ≥ 90%
- natural language prompt と formal spec の traceability coverage: ≥ 95%

**P8-C: Lean Escalation Criteria（Lean 4 エスカレーション基準）** — Planned

Z3 が `unknown` または不安定な結果を返す場合に、どの義務を Lean 4 へ送るべきかを決定論的に分類する。
既存の `lean_verified` 証明証明書ハンドシェイクと `mumei-lean` bridge を拡張し、エスカレーションの成功率を計測可能にする。

**Escalation Criteria**:

```
Escalate to Lean 4 when:
1. Z3 result == unknown / timeout / resource limit
2. 非線形算術、帰納的データ型、再帰的不変条件を含む
3. quantifier alternation または trigger-sensitive な forall/exists を含む
4. Z3 反例が P8-A で spurious_candidate と分類された
5. trusted atom を減らすために人間レビュー済み補題へ昇格する

Do not escalate when:
1. requires が unsat で仕様矛盾が原因
2. 決定可能断片内で Z3 が明確な sat 反例を返し、P8-A 再評価も成功
3. Lean 側 translator が未対応の構文で partial translation になる
```

**Implementation Plan**:

```
1. Z3 result classifier
   - timeout / unknown / sat / unsat / skipped を原因別に分類
   - proof obligation に logic fragment tag を付与

2. mumei-lean bridge integration
   - escalation candidate を proof certificate bundle として出力
   - mumei-lean/scripts/bridge.py が candidate reason を読み取り Lean proof を生成
   - Lean 結果を `z3_check_result = "lean_verified"` として戻す

3. Metrics and feedback loop
   - escalation_attempts / lean_successes / partial_translation / manual_required を記録
   - atom・logic fragment・failure reason ごとの成功率を集計
   - 低成功率カテゴリを P8-D の仕様ガイドへフィードバック
```

**Files to modify/create**:
- `mumei-core/src/proof_cert.rs` — escalation reason、logic fragment tag、Lean result metadata
- `mumei-core/src/resolver.rs` — `--allow-lean-verified` 経路での acceptance metrics
- `src/main.rs` — `mumei verify --escalate-lean` / `--emit escalation-bundle` CLI
- `mumei-lean/scripts/bridge.py` — escalation candidate bundle の取り込み
- `mumei-lean/scripts/ingest_cert.py` — candidate reason を Lean theorem metadata へ変換

**Success Metrics**:
- Z3 `unknown` obligation の Lean escalation 成功率: ≥ 70%
- partial translation 率: < 20%
- `lean_verified` certificate の再検証成功率: 100%

**P8-D: Decidable Fragment Documentation（決定可能断片ドキュメント）** — Planned

Z3 が安定して証明できる仕様の範囲を明文化し、Mumei の仕様を書く人間・AI agent の双方が「証明しやすい仕様」を選べるようにする。
これは P8-A〜C の検出・エスカレーション結果を、仕様設計のガイドラインへ還元するフェーズである。

**Documented Fragment**:

```
1. Linear arithmetic
   - i64 / Nat refinement は加減算・比較・定数倍を推奨
   - 変数同士の乗算、除算、mod、指数は Lean escalation candidate

2. Array and sequence access patterns
   - 0 <= i < len(a) の明示的境界条件を必須化
   - 単一 index の read/write と length-preserving update を推奨
   - nested mutable aliasing や quantified permutation は Lean 側へ送る

3. Quantifier restrictions
   - forall は bounded range または finite collection に限定
   - exists は witness を構成できる形を推奨
   - quantifier alternation (`forall exists`, `exists forall`) は原則 escalation

4. Effects and temporal state
   - state machine は finite state + explicit transition に制限
   - path / URL / regex 制約は Z3 String Sort の既存近似範囲を明記
```

**Implementation Plan**:

```
1. docs/SPEC_GUIDE.md に決定可能断片を追加
2. mumei verify の警告として "outside_decidable_fragment" を出す
3. mumei-agent の prompt に spec-writing guideline を注入
4. P8-C metrics から証明失敗しやすい fragment を定期更新
```

**Files to modify/create**:
- `docs/SPEC_GUIDE.md` — 決定可能断片、アンチパターン、推奨仕様テンプレート
- `docs/LANGUAGE.md` — refinement / quantifier / array access の言語仕様へのリンク
- `mumei-core/src/verification.rs` — logic fragment detector と warning diagnostic
- `mcp_server.py` — agent-facing spec guideline summary の提供
- `mumei-agent/agent/prompts/` — 仕様生成プロンプトへの guideline 反映

**Success Metrics**:
- 新規仕様の `outside_decidable_fragment` 警告率: 四半期ごとに 20% 減少
- Z3 `unknown` 率: < 5%
- AI 生成仕様の first-pass verification 成功率: ≥ 85%

**P8-E: Lean Escalation Formal Translator Specification（Lean 4 エスカレーション形式変換仕様）** — Planned

P8-C のエスカレーション判定を実運用するには、Mumei の型システム・refinement type・loop invariant を Lean 4 の依存型理論へ写像する変換規則を、実装依存のスクリプトではなく形式仕様として固定する必要がある。
このフェーズでは `mumei-lean` bridge の translator contract を定義し、Z3 で `unknown` になった義務が Lean kernel で何として解釈されるかを追跡可能にする。

**Translator Specification**:

```
1. Type system mapping
   - Mumei の i64 / bool / string / array / struct / enum を Lean 4 の Int / Bool / String / List / structure / inductive へ写像
   - ownership / borrow / capability は値の性質ではなく証明コンテキスト上の仮説として表現
   - trusted atom は Lean theorem ではなく opaque axiom または explicit assumption として provenance を保持

2. Refinement type lowering
   - `{v: T | P(v)}` を Lean の subtype または predicate argument として表現
   - requires / ensures / effect_pre / effect_post を theorem statement の前提・結論へ分離
   - counterexample reconstruction で使う witness 名と Lean binder 名を proof certificate に保存

3. Loop invariant and recursion encoding
   - `while` / `for` の invariant を well-founded recursion または induction hypothesis に変換
   - variant / decreases clause がないループは partial translation として止める
   - MIR の basic block transition を Lean 側の state transition lemma へ対応づける
```

**Compiler Technology Plan**:

```
1. Typed intermediate translator IR
   - HIR/MIR から Lean 直書き文字列へ変換せず、型付き TranslatorIR を経由する
   - sort / binder / theorem goal / provenance span を保持し、未対応構文を構造的に報告
   - generated Lean の各 declaration に source atom と proof hash を埋め込む

2. Semantic gap bridge
   - integer overflow、array bounds、string/regex、effect state の意味論差を lowering rule として明文化
   - Z3 の近似モデルと Lean の total function semantics が異なる箇所に bridge lemma を要求
   - partial translation を silent success にせず `manual_lemma_required` として分類

3. Kernel-checked escalation handshake
   - `escalation_bundle.json` → Lean source → `.olean` / result certificate の一方向パイプラインを固定
   - Lean 成功結果には theorem name、translator version、bridge lemma set hash を含める
   - translator version mismatch 時は証明キャッシュを無効化する
```

**Files to modify/create**:
- `mumei-core/src/proof_cert.rs` — translator version、binder mapping、bridge lemma hash、manual lemma reason の証明書フィールド
- `mumei-core/src/verification.rs` — HIR/MIR obligation から escalation bundle への型付き出力
- `mumei-lean/scripts/expr_translator.py` — Mumei 型・refinement・loop invariant から Lean expression への仕様準拠 translator
- `mumei-lean/scripts/ingest_cert.py` — TranslatorIR metadata を Lean declaration / theorem statement へ反映
- `mumei-lean/MumeiLean/Basic.lean` — 基本型、subtype、配列境界、effect state の bridge lemma
- `docs/PROOF_CERTIFICATE.md` — Lean escalation translator contract と certificate schema

**Success Metrics**:
- Z3 `unknown` obligation の translator 完全変換率: ≥ 80%
- Lean escalation 成功率（partial translation を除く）: ≥ 75%
- translator version mismatch による stale certificate acceptance: 0 件
- manual lemma required の reason attribution coverage: 100%

**P8-F: MCP Server Z3 Process State Management（MCP サーバー Z3 プロセス状態管理）** — Implemented

`mcp_server.py` が複数の AI agent・IDE・CI から並列に検証要求を受けると、Z3 process、verification cache、proof certificate の状態が衝突し、同じ atom の異なる義務が混線するリスクがある。
このフェーズでは MCP サーバーを単なる CLI wrapper ではなく、Z3 process lifecycle と cache isolation を管理する検証オーケストレータとして強化する。

**Implementation Plan**:

```
1. Z3 process lifecycle management
   - request ごとに solver context / timeout / memory limit / cancellation token を割り当てる
   - 長時間実行・hung process を watchdog で終了し、proof certificate に timeout reason を記録
   - warm pool を使う場合でも context reset と assertion leak detection を必須化

2. Cache conflict handling
   - cache key を source hash + dependency hash + translator version + solver config + target fragment で構成
   - 同一 key の並列書き込みは atomic write + file lock + generation id で直列化
   - stale / partial / failed cache entry を区別し、unknown を成功キャッシュとして扱わない

3. Parallel verification safety
   - verification task id を全ログ・証明書・MCP response に伝搬
   - 複数 task が同じ artifact path を更新する場合は per-module workspace へ分離
   - cancellation / retry / escalation が他 task の Z3 context や cache entry を破壊しないことをテストする
```

**Files to modify/create**:
- `mcp_server.py` — task registry、Z3 worker lifecycle、cancellation、cache lock orchestration
- `mumei-core/src/verification.rs` — solver config fingerprint、task id、timeout/cancel reason の結果伝搬
- `mumei-core/src/proof_cert.rs` — cache key、generation id、solver process metadata、parallel safety diagnostics
- `src/main.rs` — `mumei verify --task-id` / `--solver-timeout` / `--cache-scope` CLI オプション
- `tests/` — 並列 MCP 検証、cache collision、hung Z3 process、cancellation の回帰テスト
- `docs/TOOLCHAIN.md` — MCP 経由の並列検証と cache isolation の運用ガイド

**Success Metrics**:
- 100 並列 verification request で cache corruption: 0 件
- hung Z3 process の watchdog recovery 成功率: 100%
- 同一 atom の競合検証で task id / certificate provenance の取り違え: 0 件
- cache hit correctness（hash mismatch acceptance）: 100% rejected

**P8-G: Retry Budget Theoretical Foundation（リトライ予算の理論的基盤）** — Implemented

Self-healing loop と Lean escalation は成功率を上げる一方で、無制限に retry・prompt 修正・solver 再実行を許すと探索空間と token cost が爆発する。
このフェーズでは retry budget を経験則ではなく、探索木・検証義務分類・期待改善率に基づく制御問題として定式化する。

**Theoretical Boundary**:

```
1. Search space model
   - 各 repair attempt を branching factor b、depth d、solver outcome distribution を持つ探索木としてモデル化
   - retry は仕様変更・実装修正・補題追加・Lean escalation の action class に分類
   - 同一 counterexample signature への再試行は情報利得がない限り depth を増やさない

2. Formal stop conditions
   - max_attempts、max_tokens、max_solver_time、max_semantic_delta を proof task ごとに明示
   - 仕様を弱める repair は monotonicity check に通らない限り budget 消費後に human review へ送る
   - unknown → retry の回数は logic fragment と P8-E translator coverage に応じて上限を変える

3. Cost-success trade-off analysis
   - attempt n の expected marginal success rate が token/solver cost threshold を下回る場合に停止
   - high-assurance target では token cost より false proof / spec drift risk を優先
   - library proliferation では proof health gain per token を最適化指標にする
```

**Implementation Plan**:

```
1. Retry budget policy schema
   - forge task / MCP request / CLI verification に共通の BudgetPolicy を定義
   - action class ごとの token・solver・Lean escalation・semantic delta 上限を設定可能にする
   - policy fingerprint を proof certificate と agent log に保存

2. Budget-aware self-healing loop
   - Z3 counterexample signature、unsat core、Lean error class を retry state に記録
   - 同じ失敗原因への prompt 再投入を抑制し、別 action class への切り替えを明示
   - budget exhaustion 時は `manual_review_required` と structured summary を返す

3. Metrics feedback
   - attempts_to_success、tokens_to_success、solver_seconds_to_success、spec_drift_score を集計
   - fragment / task type / repair strategy ごとに Pareto frontier を可視化
   - 四半期ごとに default budget を実測データから再調整する
```

**Files to modify/create**:
- `mcp_server.py` — MCP request の BudgetPolicy 入力、retry state、budget exhaustion response
- `mumei-core/src/proof_cert.rs` — retry policy fingerprint、attempt summary、cost/success metrics の証明書フィールド
- `mumei-core/src/verification.rs` — solver retry reason、counterexample signature、semantic delta guard の結果出力
- `mumei-agent/agent/strategies/` — self-healing loop の budget-aware strategy selection
- `mumei-agent/agent/prompts/` — retry 境界と spec weakening 禁止条件の prompt 注入
- `docs/CROSS_PROJECT_ROADMAP.md` — mumei / mumei-agent / mumei-lean をまたぐ retry budget 運用計画
- `docs/PROOF_CERTIFICATE.md` — retry metrics と budget policy schema

**Success Metrics**:
- retry budget exhaustion 時の structured failure summary 出力率: 100%
- token cost あたり first-pass + retry success rate の四半期改善: ≥ 15%
- 同一 counterexample signature への無情報 retry 削減率: ≥ 80%
- spec weakening による false success regression: 0 件

---

### P9: NLAE Integration - Provable AI Runtime

Anthropic の Natural Language Autoencoders (NLAE) 理論を mumei エコシステムに統合し、LLM の推論（内部状態）と形式検証（数学的真理）をシームレスに結合する証明可能な AI 実行基盤を構築する。

#### 設計思想

Mumei DSL を、AI にとっての究極の NLA（Natural Language Activation：高密度論理言語）として位置づける。自然言語の仕様が持つ「曖昧さ（ノイズ）」を排し、AI の設計意図を 100% の忠実度（Fidelity）で数学的証明空間へ射影（コンパイル）する。

#### コンポーネントマッピング

| リポジトリ | NLAE 役割 | 具体的抽象化レイヤー |
| --- | --- | --- |
| `mumei-agent` | **Module A (AV)** | 内部推論（潜在空間） → `mumei` 構文（離散表現）への写像 |
| `mumei` | **Module B (AR)** | `mumei` 構文 → Z3 意味論（論理状態）への再構築 |
| `mumei-lean` | **Fidelity Checker** | 再構築の忠実度検証（誤差がゼロであることを数学的に担保） |
| `mumei-demo` | **Evaluation Loop** | 誤差（反例）に基づく自己修復ループの実行環境 |

**P9-A: Latent-space Debugging（潜在空間デバッグ）** — Planned

既存の `LatentEncoder` / `LatentDecoder` を拡張し、より高度な潜在空間デバッグを実現する。

**Implementation Plan**:
- `LatentEncoder` の特徴抽出機能を拡張（現在は構文・意味論・効果・依存関係・契約・スコープ・検証特徴） [6-cite-7](#6-cite-7)
- `LatentDecoder` の編集戦略を拡張（現在は effect 追加・削除・型洗練・requires 強化・ensures 弱化） [6-cite-8](#6-cite-8)
- `LatentDebugStrategy` のバグ方向計算を高度化 [6-cite-9](#6-cite-9)
- フォールバック動作とユニットテスト

**Success Metrics**:
- 潜在空間デバッグの成功率: ≥ 30%（rule-based + LLM の前段）

**P9-B: Dense Property Generation（高密度プロパティ生成）** — Planned

既存の `DensePropertyGenerator` を拡張し、より高密度な契約生成を実現する。

**Implementation Plan**:
- `DensePropertyGenerator` の LLM プロンプトを最適化 [6-cite-10](#6-cite-10)
- Z3 検証効率を考慮した契約圧縮アルゴリズム
- 生成された契約の検証時間メトリクス

**Success Metrics**:
- 生成された契約の Z3 検証時間が既存契約より 20% 短縮

**P9-C: Latent Protocol for Agent Communication（エージェント間通信プロトコル）** — Planned

既存の `LatentProtocol` を拡張し、エージェント間の効率的な通信を実現する。

**Implementation Plan**:
- `LatentProtocol` のハッシュベースエンコーディングを拡張 [6-cite-11](#6-cite-11)
- MCP サーバーへの `send_latent_message` 統合
- プロトコルのセキュリティとプライバシー保証

**Success Metrics**:
- エージェント間通信のデータ転送量が 50% 削減

**P9-D: Reconstruction Loss Formalization（復元誤差の定式化）** — Planned

プログラム状態の写像と復元誤差を数学的に定義する。

**Implementation Plan**:
- 意図される正当な仕様空間 $S$ と実装空間 $V$ の定義
- 復元誤差 $L_{\text{recon}} = \{ x \in S \mid V(x) \neq \text{True} \}$ の実装
- Z3 反例を復元誤差として解釈するモジュール
- 誤差がゼロ（$L_{\text{recon}} = \emptyset$）の状態を検証するメカニズム

**Success Metrics**:
- 復元誤差の検出精度: ≥ 95%

**P9-E: Structured Feedback JSON Schema（構造化フィードバック JSON 規格）** — Planned

AI が解釈しやすい構造化 JSON（Loss Vector）の規格を定義・実装する。

**Implementation Plan**:
- 以下の JSON スキーマの定義と実装:

```json
{
  "status": "verification_failed",
  "error_type": "postcondition_violation",
  "location": { "file": "vault.mu", "line": 12 },
  "reconstruction_loss": {
    "violated_property": "ensures from_after == from - amount",
    "counter_example": { "from": 100, "to": 0, "amount": -50, "from_after": 150 }
  },
  "feedback_instruction": "The system allowed a negative amount deposit..."
}
```

- `mumei-core` の `verification.rs` からの出力拡張
- `mumei-agent` での解釈ロジック実装

**Success Metrics**:
- AI によるフィードバック解釈成功率: ≥ 90%

**P9-F: Self-Correction Protocol（自己修復ループ）** — Planned

誤差（反例）を最小化する自律サイクルを実装する。

**Implementation Plan**:
- 生成 → 検証 → 反例出力 → 修正 → 証明のループ実装
- `mumei-demo` での評価環境構築
- ループの収束条件と停止条件の定義
- トークンコストと成功率のトレードオフ最適化

**Success Metrics**:
- 自己修復ループの収束率: ≥ 70%（10 回以内）

**P9-G: Ecosystem Integration（エコシステム統合）** — Planned

4 つのリポジトリを NLAE コンポーネントとして統合する。

**Implementation Plan**:
- `mumei-core`: Z3 変換時の Loss Vector 出力モジュール
- `mumei-agent`: MCP 経由の JSON エラー入力と NLAE Backpropagation プロンプト
- `mumei-lean`: 完全証明のための Lean 4 トランスパイル
- `mumei-demo`: 自己修復ループの実行環境

**Success Metrics**:
- エンドツーエンドの NLAE 統合デモの成功

#### Configuration

- すべての機能はデフォルト無効（既存の NLAE 機能と同様）
- `ENABLE_LATENT_DEBUG`, `ENABLE_DENSE_PROPERTIES`, `ENABLE_LATENT_PROTOCOL`
- `ENABLE_RECONSTRUCTION_LOSS`, `ENABLE_STRUCTURED_FEEDBACK`, `ENABLE_SELF_CORRECTION`

#### References

- Anthropic NLAE research: https://www.anthropic.com/research/natural-language-autoencoders
- Reference implementation: https://github.com/kitft/natural_language_autoencoders
- Existing NLAE integration: `mumei-agent/docs/NLAE_INTEGRATION.md`

---

## Related Documents

- [`docs/FFI.md`](FFI.md) — FFI extern block design (Phase A foundation)
- [`docs/CONCURRENCY.md`](CONCURRENCY.md) — Structured concurrency (Phase D foundation)
- [`docs/STDLIB.md`](STDLIB.md) — Standard library reference (Phase B/C additions)
- [`docs/TOOLCHAIN.md`](TOOLCHAIN.md) — CLI commands and distribution
- [`instruction.md`](../instruction.md) — Development guidelines and priorities
- [`docs/CROSS_PROJECT_ROADMAP.md`](CROSS_PROJECT_ROADMAP.md) — Cross-project roadmap for mumei + mumei-agent (2026-03〜)
