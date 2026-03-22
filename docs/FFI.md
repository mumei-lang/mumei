# FFI (Foreign Function Interface) Design Document

> Mumei's FFI-first design and Bridge mechanism.

## Overview

Mumei adopts an **FFI-first** design philosophy, providing a foreign function interface
for safe interoperation with existing Rust and C ecosystems.
FFI functions are auto-registered as `trusted atom`s and verified only via contracts
(requires/ensures) — body is externally implemented.

## extern Block Syntax

### Basic Syntax

```mumei
extern "Rust" {
    fn sqrt(x: f64) -> f64;
    fn abs(x: i64) -> i64;
}
```

### Syntax Elements

| Element | Description |
|---|---|
| `extern` | Keyword to start an external function block |
| `"Rust"` / `"C"` | Target language name |
| `fn name(params) -> RetType;` | Function signature declaration |

### Verified FFI Contracts

Extern functions can optionally declare `requires` and `ensures` contracts.
These contracts are **not** body-verified (the body is external), but they **are**
checked at every call site by Z3 — the caller must satisfy `requires`, and may
assume `ensures` holds after the call.

```mumei
extern "Rust" {
    fn sqrt(x: f64) -> f64
        requires: x >= 0.0;
        ensures: result >= 0.0;
}
```

If no contracts are specified, they default to `true` (backward compatible).

### AST Representation (implemented)

```rust
pub struct ExternFn {
    pub name: String,
    pub param_types: Vec<String>,
    pub return_type: String,
    pub requires: Option<String>,   // verified FFI contract
    pub ensures: Option<String>,    // verified FFI contract
    pub span: Span,                 // source location
}

pub struct ExternBlock {
    pub language: String,
    pub functions: Vec<ExternFn>,
    pub span: Span,             // source location
}
```

Included as `Item::ExternBlock(ExternBlock)` in `parse_module` results.
Handled in all match blocks in `main.rs`, `resolver.rs`, `lsp.rs`.

## Implementation Status

| Item | Status |
|---|---|
| extern block syntax parsing | ✅ Implemented |
| Verified FFI contracts (`requires`/`ensures` on extern fns) | ✅ Implemented |
| `ExternFn` / `ExternBlock` AST | ✅ Implemented (with Span + contracts) |
| `Item::ExternBlock` variant | ✅ Implemented (all match arms) |
| Parser tests | ✅ Implemented (`test_parse_extern_block`, `test_parse_extern_block_c`) |
| trusted atom auto-registration | ✅ Implemented (PR #32: extern → ModuleEnv auto-registration) |
| LLVM codegen | ✅ Implemented (`declare_extern_functions()` + `resolve_return_type()`) |
| FFI memory management | ✅ Implemented (`json_free`, `string_free`, `http_free`) |
| Managed string lifetime | ✅ Implemented (`mumei_str_alloc`, `mumei_str_free`, `mumei_str_get`) |

## Bridge Mechanism (Design)

### Integration with trusted atom

Functions declared in `extern` blocks are auto-registered as `trusted atom`s:

1. **Body verification skip**: External implementation, so Z3 body verification is skipped
2. **Contract verification**: `requires` / `ensures` contracts are verified at call sites
3. **Taint analysis**: `trusted` function return values are tagged with `__tainted_` markers

> NOTE: `ExternFn` fields were previously `#[allow(dead_code)]` but are now used
> for trusted atom auto-registration in `load_and_prepare()`.

### Usage Example

```mumei
extern "Rust" {
    fn sqrt(x: f64) -> f64;
}

atom safe_sqrt(x: f64) -> f64
    requires: x >= 0.0;
    ensures: result >= 0.0;
    body: sqrt(x);
```

## Supported Languages

| Language | Status | Description |
|---|---|---|
| Rust | Designed | References `extern "C"` symbols from Rust crates |
| C | Designed | References function symbols from C libraries |

## Parsing

extern blocks are detected in `parse_module()` using the following regex:

```
extern\s+"(\w+)"\s*\{([^}]*)\}
```

Each function signature is extracted with:

```
fn\s+(\w+)\s*\(([^)]*)\)\s*->\s*(\w+)
```

## Future Extensions

> Details: [`docs/ROADMAP.md`](ROADMAP.md) Phase P1-A

### 🥇 Priority 1: FFI Bridge Completion (Roadmap P1-A)

Completing the FFI Bridge is the top priority as a prerequisite for std.http / std.json.

**Implementation Plan**:

1. **ExternBlock → trusted atom auto-conversion** ✅ (PR #32)
   - Generate `Atom` from `ExternFn` signature
   - Set `TrustLevel::Trusted` (skip body verification)
   - Auto-register in `ModuleEnv.atoms`

2. **LLVM declare generation** ✅
   - `declare_extern_functions()` emits LLVM IR `declare` for all extern functions
   - `resolve_param_type()` / `resolve_return_type()` map Mumei types → LLVM types

3. **Call-site code generation** ✅
   - Callee return type resolved from `atom.return_type` annotation
   - ABI: both "C" and "Rust" use C calling convention

4. **Memory management** ✅ (Plan 16)
   - `json_free()` / `string_free()` / `http_free()` release handles from global stores
   - `mumei_str_alloc()` / `mumei_str_free()` / `mumei_str_get()` for managed string lifetime
   - Exposed as atoms in `std/json.mm` and `std/http.mm`

**Files modified**:
- `src/main.rs` — ExternBlock → atom conversion in `load_and_prepare()`
- `src/verification.rs` — trusted verification for extern atoms
- `src/codegen.rs` — `declare_extern_functions()`, `resolve_return_type()`, LLVM `declare` + `call` generation
- `src/ffi/json.rs` — JSON FFI backend + memory management (`json_free`, `string_free`, `mumei_str_alloc/free/get`)
- `src/ffi/http.rs` — HTTP FFI backend + memory management (`http_free`)

### Other Extensions

1. **std.http backend** (Roadmap P1-C): HTTP client wrapping reqwest via FFI
2. **std.json backend** (Roadmap P1-B): JSON operations wrapping serde_json via FFI
3. **std hierarchy**: Reorganize into `std.core` / `std.net` / `std.math` module hierarchy
4. **Link directives**: `#[link(name = "libm")]` equivalent linker directive syntax
5. **Type mapping**: Automatic mapping between Mumei types and foreign language types

## Related Files

- `src/parser.rs` — `ExternFn`, `ExternBlock` struct definitions + parsing
- `src/verification.rs` — Verification skip via `TrustLevel::Trusted`
- `src/codegen.rs` — External function call generation in LLVM IR (pending)
