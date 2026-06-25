# LSP Integration

The Mumei language server (`mumei lsp`) provides editor-level visibility into verification feedback, contracts, and intent/spec alignment for `.mm` files.

## Capabilities

During `initialize`, the server advertises:

- `textDocumentSync` for full-document updates
- `hoverProvider` for atom contract summaries
- `completionProvider` for keywords and parsed item names
- `definitionProvider` for source jumps
- `codeLensProvider` with `resolveProvider: true` for inline intent/spec metadata

## CodeLens Features

`textDocument/codeLens` returns lenses for each parsed atom, including atoms inside `impl` blocks.

### Intent Drift Score

Atom definition lines show:

```text
Intent Drift: 0.42
```

The current score is a lightweight heuristic derived from:

- `requires` and `ensures` constraint complexity
- quantifier count
- body/control-flow/operator complexity
- effects, resources, and effect state transitions
- parameter, generic, and trait-bound count

Scores are normalized to `0.00` through `1.00`:

| Score | Meaning |
|---|---|
| `0.00` - `0.30` | Low drift risk; implementation and contract are simple. |
| `0.31` - `0.74` | Medium drift risk; review the contract/body relationship. |
| `0.75` - `1.00` | High drift risk; the LSP also emits a warning diagnostic. |

The CodeLens command is `mumei.showIntentDrift` with arguments:

```json
["file:///path/to/file.mm", "atom_name", 0.42]
```

This command is intended for editor extensions to open a detail dashboard. Future versions can replace the heuristic with `mumei-agent`'s `IntentTracker` output while keeping the same LSP surface.

### Spec-Code Mapping

Each `requires:` and `ensures:` clause shows a mapping lens:

```text
Spec-Code Mapping: requires → atom_name
Spec-Code Mapping: ensures → atom_name
```

The CodeLens command is `mumei.showSpecCodeMapping` with arguments:

```json
["file:///path/to/file.mm", "atom_name", "requires"]
```

Editor extensions can use this to open a side panel that connects the selected contract clause to the atom implementation, diagnostics, counter-examples, and proof artifacts.

## Diagnostics

The LSP appends intent drift warnings to normal parse/Z3 diagnostics when a score is at least `0.75`:

```text
High intent drift score (0.82) for atom 'transfer'. Review spec-code mapping.
```

The diagnostic `data` field includes:

```json
{
  "intentDrift": 0.82,
  "atom": "transfer"
}
```

For `.mm` files, comments beginning with `/// spec:` are sent to
`mumei-agent validate-spec --format json`. Returned `spec_health_issues` become
diagnostics on the original comment lines, with `next_steps` attached as the
human-review handoff rather than renamed into a separate recommendation bucket.

For foreign code, opening a `.py`, `.rs`, or `.go` document runs
`mumei-agent validate-code --input <path> --language <language>` when available.
Returned `verification_violations` and `cross_validation_gaps` are shown inline
using the source line/column metadata from the JSON payload.

If `mumei-agent` is not installed or returns malformed JSON, the server
gracefully degrades: `.mm` documents still receive the existing parse/Z3
diagnostics, and foreign-code agent diagnostics are omitted.

## VS Code Extension Hooks

The bundled VS Code extension registers:

- `mumei.showIntentDrift`: opens an intent drift detail dashboard for the provided atom.
- `mumei.showSpecCodeMapping`: opens a side panel focused on the selected `requires` or `ensures` clause.

The LSP client requests CodeLens for `.mm` files and refreshes lenses after document edits so scores update with the current source text.

## Usage

1. Start the server with `mumei lsp`.
2. Open a `.mm` file in an editor configured for the Mumei LSP.
3. Confirm atom definitions show `Intent Drift` CodeLens entries.
4. Confirm `requires:` and `ensures:` lines show `Spec-Code Mapping` entries.
5. Click the lenses in an editor extension that implements the corresponding commands.
