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
- `src/verification.rs` — trusted verification for extern atoms
- `src/codegen.rs` — LLVM `declare` + `call` generation
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
- `src/parser.rs` — string literal type extension (if needed)
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
1. Create mumei-lang/homebrew-mumei repository
2. Create Formula (download from GitHub Releases)
3. Auto-update Formula via CI (release.yml integration)
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
- Regex-based path policies: `matches(path, "/tmp/[a-z]+\\.txt")`
- URL validation for std.http effects: `starts_with(url, "https://")`

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

### Why Phases 2–5 Are Deferred

- **Phase 2 (Basic Effects)**: ✅ Complete — parameterized effects (`FileRead(path: Str)`, `HttpGet(url: Str)`) implemented with security policy enforcement. Standard library effects defined in `std/effects.mm`, `std/http.mm`, `std/file.mm`. Z3 verifies parameter constraints (e.g., `starts_with(path, "/tmp/")`) at compile time.
- **Phase 3 (Effect Polymorphism)**: ✅ Complete — Effect polymorphism via `<E: Effect>` bounds and `with E` syntax. Resolved through monomorphization (same as type polymorphism).
- **Phase 4 (MIR)**: A CFG-based intermediate representation is needed for borrow checking and lifetime analysis, but the borrow checking design itself is not yet started. Will be introduced after the design is finalized.
- **Phase 5 (HIR Effect Type Information)**: ✅ Complete — `HirEffectSet` attached to `HirAtom`, `HirExpr::Call`, `HirExpr::Perform`. `lower_atom_to_hir_with_env()` populates effect info from `ModuleEnv`. Codegen and all transpilers read effects from `hir_atom.effect_set`.
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
| 7 | Runtime Portability: musl + Windows | ✅ CI infrastructure added (untested on runners) |
| 8 | Concurrency improvements | ✅ Parser/AST/HIR infrastructure added (codegen placeholder) |
| 9 | Plan 15: Examples + E2E tests | ✅ 5 examples + 3 test files |
| 10 | Plan 16: FFI memory management | ✅ json_free/string_free/http_free |
| 11 | Plan 17: Str type migration | ✅ Examples use Str-typed parameters |
| 12 | Plan 18: Codegen return types | ✅ `resolve_return_type()`, `-> Type` syntax |
| 13 | Plan 19: MIR Phase 4c completion | ✅ MoveAnalysis is primary engine |
| 14 | Plan 20: Z3 temporal effect integration | ✅ `encode_effect_state()`, ConflictingState Z3 probes |
| 15 | Plan 21: Verified HTTP Server + Path Safety | ✅ SafeFileRead/SafeFileWrite effects, `&&` compound constraints, HTTP server FFI, HttpServer stateful effect, path traversal prevention demo |
| 16 | Plan 22: PII Pipeline Example | ✅ DataPipeline temporal effect demo + E2E tests |

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

---

## Related Documents

- [`docs/FFI.md`](FFI.md) — FFI extern block design (Phase A foundation)
- [`docs/CONCURRENCY.md`](CONCURRENCY.md) — Structured concurrency (Phase D foundation)
- [`docs/STDLIB.md`](STDLIB.md) — Standard library reference (Phase B/C additions)
- [`docs/TOOLCHAIN.md`](TOOLCHAIN.md) — CLI commands and distribution
- [`instruction.md`](../instruction.md) — Development guidelines and priorities
