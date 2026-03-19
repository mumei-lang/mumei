# Diagnostics Design Document

> Diagnostics infrastructure design for the Mumei compiler.

## Overview

The Mumei compiler adopts a diagnostics-driven design where source location information (`Span`) is attached to all AST nodes, providing accurate and actionable error messages to developers.

## Implementation Status

| Item | Status |
|---|---|
| `Span` struct | ✅ Implemented (`src/parser.rs`) |
| `span` field on all AST Item types | ✅ Implemented (Atom, StructDef, EnumDef, TraitDef, ImplDef, ResourceDef, ImportDecl, RefinedType, ExternBlock, ExternFn) |
| `MumeiError` with Span integration | ✅ Implemented (`VerificationError { msg, span, original_span }` etc.) |
| `ErrorDetail` struct | ✅ Implemented (message + span + suggestion) |
| `offset_to_line_col` helper | ✅ Implemented (computes line/col from regex match offset) |
| LSP diagnostics with Span | ✅ Implemented (`lsp.rs` `find_error_position`) |
| Rich Diagnostics (miette) | ✅ Implemented (colored output, source highlighting, actionable suggestions via `miette` crate) |

## Span Information

### Structure

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub len: usize,
}
```

| Field | Description |
|---|---|
| `file` | Source file path (empty string = unknown) |
| `line` | Line number (1-indexed, 0 = unknown) |
| `col` | Column number (1-indexed, 0 = unknown) |
| `len` | Token length in characters (0 = unknown) |

### Coverage

Span is attached to all AST types:

- `Atom` — function definitions
- `StructDef`, `EnumDef` — type definitions
- `TraitDef`, `ImplDef` — trait/impl definitions
- `ResourceDef` — resource definitions
- `ImportDecl` — import declarations
- `RefinedType` — refinement type definitions
- `ExternBlock`, `ExternFn` — FFI external function declarations

## Error Reporting Strategy

### Error Type (miette::Diagnostic compatible)

```rust
#[derive(thiserror::Error, miette::Diagnostic, Debug)]
pub enum MumeiError {
    #[error("Verification Error: {msg}")]
    #[diagnostic(code(mumei::verification))]
    VerificationError {
        msg: String,
        #[source_code]
        src: miette::NamedSource<String>,
        #[label("verification failed here")]
        span: miette::SourceSpan,
        #[help]
        help: Option<String>,
        /// Original parser::Span (line/col) preserved for LSP
        original_span: Span,
    },
    // CodegenError, TypeError follow the same structure
}
```

Constructors:
- `MumeiError::verification(msg)` — no Span (unknown location)
- `MumeiError::verification_at(msg, span)` — with Span
- `MumeiError::verification_with_source(msg, span, source, help)` — rich output with source code
- `MumeiError::type_error_at(msg, span)` — type error + Span
- `.with_source(source, span)` — attach source code to existing error
- `.with_help(msg)` — attach help message

Helper functions:
- `span_to_source_span(source, span)` — converts `Span` (line/col/len) to `miette::SourceSpan` (byte offset), handles both `\n` and `\r\n` line endings

### LSP Integration (implemented)

`lsp.rs` `verify_source_for_lsp` converts Z3 verification errors to `ErrorDetail` and sends precise line/column positions via `publishDiagnostics`:

```
textDocument/publishDiagnostics:
  - range: converted from ErrorDetail.span (1-indexed → 0-indexed)
  - message: ErrorDetail Display output
  - severity: 1 (Error)
```

### Rich Diagnostics Output Examples (miette)

Examples of rich error output powered by miette:

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

```
  × Verification Error: Potential division by zero.
  help: Add `divisor != 0` condition to requires.
```

```
  × Verification Error: Call to 'foo': precondition (requires) not satisfied at call site
  help: The precondition is not satisfied at the call site. Check the argument constraints.
```

### Future Extensions

1. **Multi-span**: ✅ Implemented — Display multiple related source locations for a single error using miette's `#[related]` field and `RelatedDiagnostic` struct. Supports propagation through `with_source()` and LSP `relatedInformation`.
2. **Compound Constraint Decomposition**: ✅ Implemented — `&&`-joined constraints are split and individually explained with satisfied/violated status via `split_compound_constraint()` and `evaluate_sub_constraint()`.
3. **Expression-Level Dataflow Tracking**: ✅ Implemented — `Span` added to `Stmt` variants, `DataFlowEntry` struct for tracking data flow chains, included in semantic feedback JSON.
4. **Snapshot tests**: Regression tests for miette output formatting
5. **Enhanced LSP integration**: ✅ Implemented — LSP diagnostics include `relatedInformation` for multi-location errors via `build_related_information()` in `lsp.rs`.
6. **RelatedDiagnostic precise line mapping**: `RelatedDiagnostic` currently stores only `miette::SourceSpan` (byte offset). In the LSP path, source text is not propagated to related diagnostics, so `relatedInformation` positions fall back to (0, 0). Fix: add `original_span: parser::Span` to `RelatedDiagnostic` and use it directly in `build_related_information()`. This also enables `with_source()` to selectively propagate source only to same-file related spans, preserving cross-file context.
7. **Cross-file related diagnostics**: `with_source()` currently overwrites all `RelatedDiagnostic` entries with the primary file's source, losing cross-file span information. Requires `original_span` on `RelatedDiagnostic` to conditionally propagate.

## Design Principles

1. **Position info on all nodes**: Parser attaches Span to every AST node
2. **Propagation**: Span is preserved/propagated during monomorphization (generics expansion)
3. **Incremental adoption**: Span → MumeiError integration → ErrorDetail → Rich Diagnostics (miette)
4. **Backward compatibility**: Existing error messages maintained while incrementally improving
5. **Suggestion-driven**: Concrete fix suggestions displayed via `#[help]` attribute

## Related Files

- `src/parser.rs` — `Span` struct definition, `offset_to_line_col`, `span_from_offset` helpers
- `src/verification.rs` — `MumeiError`, `ErrorDetail`, `span_to_source_span`, Span-aware error constructors
- `src/lsp.rs` — `find_error_position`, `verify_source_for_lsp`
