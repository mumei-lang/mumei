# Changelog

All notable changes to the "Mumei Language" extension will be documented in this file.

## [0.3.0] — 2026-05-31

### Changed

- Prepared the Marketplace package metadata and maintainer documentation for
  the `mumei-lang.mumei` 0.3.0 release.
- Expanded the README with installation, local VSIX testing, LSP verification,
  and known-issues guidance for users and maintainers.

### Verified

- `vsce package` produces a distributable `.vsix` from `editors/vscode/`.
- Extension asset validation covers Marketplace metadata, TextMate grammar, and
  `language-configuration.json`.

## [0.2.0] — 2026-04-26

### Added

- Counter-example inline visualization: Z3 counter-example values produced by
  the Mumei verifier (e.g. `len = 0`, `divisor = 0`) are now displayed as
  italic ghost-text decorations next to verification errors, both via the
  diagnostic message and via structured `data.counterexample` payloads from
  the LSP.
- Automatic decoration refresh on `onDidChangeDiagnostics`,
  `onDidChangeActiveTextEditor`, and `onDidChangeVisibleTextEditors`, so
  counter-example annotations stay in sync as the user edits.

## [0.1.0] — 2026-04-19

Initial release prepared for the Visual Studio Code Marketplace as part of
SI-5 milestone **P8-A** (VS Code Extension → Published).

### Added

- Syntax highlighting for Mumei (`.mm`) files via TextMate grammar
  - Keywords: `atom`, `struct`, `trait`, `impl`, `extern`, `import`, `match`, `if`/`else`, etc.
  - Contract clauses: `requires`, `ensures`, `body`, `invariant`, `where`
  - Effect system: `effects`, `effect_pre`, `effect_post`, `resource`, `acquire`, `release`
  - Built-in types: `i32`, `i64`, `u32`, `u64`, `f32`, `f64`, `bool`, `String`
  - Literals: numbers, strings, booleans
  - Comments: line (`//`) and block (`/* */`)
  - Operators: `&&`, `||`, `>=`, `<=`, `==`, `!=`, `+`, `-`, `*`, `/`, `|>`, `->`, `=>`
- Language Server Protocol (LSP) client
  - Code completion
  - Go to definition
  - Real-time diagnostic display (verification errors)
- Language configuration
  - Auto-closing pairs for brackets, quotes
  - Comment toggling (`//`, `/* */`)
  - Code folding by braces
- Extension settings
  - `mumei.serverPath`: Path to the `mumei` binary (default: `"mumei"`)
