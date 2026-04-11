# Mumei Language — VS Code Extension

VS Code extension for the [Mumei](https://github.com/mumei-lang/mumei) programming language, providing syntax highlighting and Language Server Protocol (LSP) integration.

Mumei is a mathematical proof-driven language that uses Z3 to formally verify function contracts before compiling to LLVM IR.

## Features

- **Syntax Highlighting** — Full TextMate grammar for `.mm` files (keywords, types, contracts, effects, operators, comments, literals)
- **Code Completion** — Context-aware completions via LSP
- **Go to Definition** — Jump to atom, struct, trait, and type definitions
- **Real-time Diagnostics** — Verification errors and warnings displayed inline as you type

## Requirements

- The `mumei` binary must be installed and available on your `PATH`.
  - Install via Homebrew: `brew tap mumei-lang/mumei && brew install mumei`
  - Or download from [GitHub Releases](https://github.com/mumei-lang/mumei/releases)

## Extension Settings

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `mumei.serverPath` | `string` | `"mumei"` | Path to the `mumei` binary used for the language server |

If the `mumei` binary is not on your `PATH`, set the full path:

```json
{
  "mumei.serverPath": "/usr/local/bin/mumei"
}
```

## Supported Syntax

- **Declarations**: `atom`, `struct`, `trait`, `impl`, `extern`, `import`, `type`
- **Contracts**: `requires`, `ensures`, `body`, `invariant`, `where`
- **Control flow**: `if`, `else`, `match`, `for`, `while`
- **Effects**: `effects`, `effect_pre`, `effect_post`, `resource`, `acquire`, `release`
- **Built-in types**: `i32`, `i64`, `u32`, `u64`, `f32`, `f64`, `bool`, `String`

## Example

```mumei
atom increment(n: i64)
    requires: n >= 0;
    ensures: result == n + 1;
    body: n + 1;
```

<!-- TODO: Add screenshot -->

## Links

- [Mumei Language](https://github.com/mumei-lang/mumei)
- [Issue Tracker](https://github.com/mumei-lang/mumei/issues)
- [Changelog](CHANGELOG.md)

## License

Apache-2.0
