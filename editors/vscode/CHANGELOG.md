# Changelog

All notable changes to the "Mumei Language" extension will be documented in this file.

## [0.1.0] — 2024-XX-XX

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
