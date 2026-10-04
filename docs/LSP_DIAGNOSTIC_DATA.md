# LSP Diagnostic and CodeLens Data

This document is the cross-editor contract emitted by the current Mumei
language server.  Editors should render these fields as-is; this document
does not define a second vocabulary.

## Diagnostic sources

| `source` | Severity | Meaning |
| --- | ---: | --- |
| `mumei` | 1 | Mumei parse or language-server diagnostic. |
| `mumei-z3` | 1 | Z3 verification failure or a pending Lean escalation. |
| `mumei-lean` | 3 | An atom whose certificate has `z3_check_result == "lean_verified"`. |
| `mumei-agent` | 2 | Agent-reported specification health or cross-validation issue. |
| `mumei-intent` | 2 | Intent drift at or above the `0.75` threshold. |

Severity values are LSP diagnostic severities (`1` error, `2` warning,
`3` information).

## `mumei-z3` data

Z3 diagnostics may carry:

```json
{
  "counterexample": {
    "a": "1",
    "b": "2"
  },
  "counterexample_provenance": {
    "complete": true,
    "values": {
      "a": {
        "status": "raised",
        "lowering": "int",
        "source_type": "i64"
      }
    },
    "omitted_solver_symbols": []
  },
  "lean_escalation": {
    "status": "pending",
    "atom": "...",
    "z3_result_class": "...",
    "escalation_reason": null
  }
}
```

`counterexample` maps source names to rendered string values. It is also appended to the
diagnostic message as `Counter-example: a = 1, b = 2`, unless the message
already contains `Counter-example:`.  A pending escalation is also reflected
in the message suffix:
`Lean escalation: pending (z3 <class>, reason <reason>)`.
`counterexample_provenance` is optional. Its `values` object records each
reported value's `raised` or `unraisable` status, lowering, optional declared
source type and solver symbol, and an unraisable reason. `complete` is false
when any reported value could not be raised. `omitted_solver_symbols` lists
internal or otherwise unreported solver symbols.

Primitive lowering labels include `int`, signed `bitvec_i64`/`bitvec_i32`,
unsigned `bitvec_u64`/`bitvec_u32`, exact `real_f64`/`real_f32`, and
`ieee754_f64`/`ieee754_f32` for matching IEEE floating-point sorts.
`str_length` and `array_length` identify the solver-side length companions of
`Str` and array parameters (`len(s)`); they are reported in reconstruction-loss
values and provenance.

In-process verification stops at the first failing atom.  When a sibling
`<file>.proof.json` certificate exists, every other atom it records with an
`escalation_reason` and a `z3_check_result` other than `lean_verified` also
receives a severity `1` `mumei-z3` diagnostic with the same `lean_escalation`
payload plus `"certificate": "<path>"`, so all pending escalations in the file
are shown, each exactly once.

## `mumei-lean` data

Each Lean-verified atom produces a severity `3` diagnostic with:

```json
{
  "lean_escalation": {
    "status": "lean_verified",
    "atom": "...",
    "z3_result_class": "...",
    "certificate": "..."
  }
}
```

The diagnostic is emitted per atom whose certificate has
`z3_check_result == "lean_verified"`.

A kernel-axiom audit rejection produces a severity `2` diagnostic:

```json
{
  "lean_escalation": {
    "status": "axiom_rejected",
    "atom": "...",
    "z3_result_class": "...",
    "certificate": "...",
    "disallowed": ["sorryAx"]
  }
}
```

`disallowed` lists the non-standard kernel axioms reported by the rejected
`axiom_audit`. An audit error also uses `axiom_rejected` but omits `disallowed`.
This diagnostic is also emitted for an escalated atom whose certificate still
records its Z3 result but whose attached Lean result failed the audit.

## `mumei-intent` data

Intent-drift diagnostics have severity `2` and are emitted when drift is
greater than or equal to `0.75`:

```json
{
  "intentDrift": 0.0,
  "atom": "..."
}
```

## `mumei-agent` diagnostics

Agent diagnostics are produced from `spec_health_issues` and
`cross_validation_gaps`.  Where available, the message includes
`verification_status: <status>`.  Human review entry points from `next_steps`
are emitted through `relatedInformation`.

These diagnostics do not carry a `data` field today.

## CodeLens data

CodeLens entries use the following literal `kind` values:

```json
{
  "kind": "intentDrift",
  "atom": "...",
  "score": 0.0
}
```

```json
{
  "kind": "specCodeMapping",
  "atom": "...",
  "clause": "..."
}
```

## Renderer rules

Renderers reflect `lean_verified`, `escalation_reason`, `z3_result_class`, and
`disallowed` verbatim.  Do not introduce alternate names.  The only `status`
values are `pending`, `lean_verified`, and `axiom_rejected`.  Unknown keys must
be ignored, not remapped.

## Editor reference implementations

These snippets are deliberately minimal reference implementations.

### Neovim

```lua
vim.diagnostic.handlers.mumei = {
  show = function(_, bufnr, diagnostics)
    for _, diagnostic in ipairs(diagnostics) do
      local data = diagnostic.user_data
        and diagnostic.user_data.lsp
        and diagnostic.user_data.lsp.data
      -- Render data.counterexample and data.lean_escalation as documented.
    end
  end,
}
```

### Emacs / eglot and flymake

```elisp
(add-hook 'eglot-managed-mode-hook
          (lambda ()
            (flymake-log :warning "Read LSP diagnostic data from the eglot diagnostic")))
```

An eglot client should read the diagnostic data attached to the flymake
diagnostic and apply the same renderer rules above.

### JetBrains / LSP4IJ

```java
void renderMumeiData(JsonObject data) {
  // Read counterexample, lean_escalation, and CodeLens kind without renaming.
}
```
