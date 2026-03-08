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
- ❌ LLVM codegen (extern function declare + call)

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
- ❌ musl target (fully static linking)
- ❌ Windows binaries

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
- **Hybrid Path Verification**: Constant Folding (Rust-side) + Symbolic String ID (Z3 Int)
- **Effect Hierarchy (Subtyping)**: `parent:` field on EffectDef enables Network → HttpRead/TcpConnect
- **MCP Pre-check**: `get_inferred_effects` tool lets AI check required permissions before writing code

### TODO: Z3 String Sort Migration

When the hybrid approach reaches its limits (complex string operations, regex matching, concatenation verification), migrate to Z3's native String sort:

**Prerequisites**:
- `z3` crate version 0.12+ with stable `z3::ast::String` support
- `libz3` static linking to avoid environment dependency issues
- Performance benchmarking: String sort is significantly slower than Int sort

**Migration plan**:
1. Add `Str` to mumei's Type system (`TypeRef::Str`)
2. Lift string literals to `z3::ast::String::from_str` in constraint generation
3. Convert `starts_with(path, "/tmp/")` to Z3's `str.prefixof` operation
4. Convert `contains(path, "..")` to Z3's `str.contains` operation
5. Benchmark AI-in-the-loop latency (target: < 500ms for typical path constraints)

**Unlocks**:
- Free-form path construction: `"/tmp/" + user_id + "/log.txt"` verification
- Regex-based path policies: `matches(path, "/tmp/[a-z]+\\.txt")`
- URL validation for std.http effects: `starts_with(url, "https://")`

### TODO: Effect Hierarchy Extensions

Future extensions to the effect subtyping system:

1. **Multi-parent (Intersection)**: `effect SecureNetRead parent: [Network, Encrypted];`
2. **Effect polymorphism**: `atom pipe<E: Effect>(f: atom_ref(T) -> U with E)` — generic over effect sets
3. **Effect narrowing**: When calling a `Network`-annotated function with only `HttpRead`, narrow the effect at the call site
4. **Negative effects**: `atom pure_compute() effects: [!IO];` — explicitly deny effects
5. **Effect aliases**: `effect IO = FileRead | FileWrite | ConsoleOut;` — union types for convenience

---

## Multi-Stage IR Roadmap

| Phase | Item | Status | Prerequisite |
|---|---|---|---|
| Phase 0 | Expr/Stmt separation | ✅ Done | — |
| Phase 1 | HIR introduction (typed AST, eliminate String-based body_expr) | ✅ Done | Phase 0 |
| Phase 2 | Basic Effect System | ⏳ Planned | Migrate parser away from regex |
| Phase 3 | Effect Polymorphism | ⏳ Planned | Phase 2 |
| Phase 4 | MIR introduction (CFG for borrow checking) | ⏳ Planned | Borrow checking design finalized |
| Phase 5 | Capability Security evaluation | ⏳ Planned | Phase 3 maturity assessment |

### Why Phases 2–5 Are Deferred

- **Phase 2 (Basic Effects)**: The current parser is regex-based and cannot safely handle complex syntax like `<E: Effect>`. Migration to a recursive descent parser must come first.
- **Phase 3 (Effect Polymorphism)**: Adding polymorphism before concrete effect definition and tracking infrastructure exists is premature. Requires basic effect implementation and operational experience.
- **Phase 4 (MIR)**: A CFG-based intermediate representation is needed for borrow checking and lifetime analysis, but the borrow checking design itself is not yet started. Will be introduced after the design is finalized.
- **Phase 5 (Capability Security)**: Evaluate the maturity of the current approach (verifying parameterized effects with Z3). If insufficient, introduce an object-based capability model.

---

## Related Documents

- [`docs/FFI.md`](FFI.md) — FFI extern block design (Phase A foundation)
- [`docs/CONCURRENCY.md`](CONCURRENCY.md) — Structured concurrency (Phase D foundation)
- [`docs/STDLIB.md`](STDLIB.md) — Standard library reference (Phase B/C additions)
- [`docs/TOOLCHAIN.md`](TOOLCHAIN.md) — CLI commands and distribution
- [`instruction.md`](../instruction.md) — Development guidelines and priorities
