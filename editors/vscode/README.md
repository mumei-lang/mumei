# Mumei Language — VS Code Extension

VS Code extension for the [Mumei](https://github.com/mumei-lang/mumei) programming language, providing syntax highlighting and Language Server Protocol (LSP) integration.

Mumei is a mathematical proof-driven language that uses Z3 to formally verify function contracts before compiling to LLVM IR.

## Features

- **Syntax Highlighting** — Full TextMate grammar for `.mm` files (keywords, types, contracts, effects, operators, comments, literals)
- **Code Completion** — Context-aware completions via LSP
- **Go to Definition** — Jump to atom, struct, trait, and type definitions
- **Real-time Diagnostics** — Verification errors and warnings displayed inline as you type
- **Intent CodeLens** — Inline intent drift scores and spec-code mapping panels for `requires`/`ensures`

## Installation

### From the VS Code Marketplace

1. Install the [Mumei Language extension](https://marketplace.visualstudio.com/items?itemName=mumei-lang.mumei).
2. Install the `mumei` CLI and make sure it is available on `PATH`.
3. Open a `.mm` file. The extension starts `mumei lsp` automatically.

### From a local VSIX

Maintainers and testers can install the packaged extension before publishing:

```bash
cd editors/vscode
npm install
npm run compile
npx vsce package --out mumei.vsix
code --install-extension mumei.vsix
```

In VS Code, you can also run **Extensions: Install from VSIX...** and select
`editors/vscode/mumei.vsix`.

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

## Usage

1. Open a Mumei source file (`*.mm`).
2. Use completions after typing Mumei keywords, atom names, types, or effects.
3. Run **Go to Definition** on atom/type/effect names defined in open Mumei
   documents.
4. Review diagnostics emitted by `mumei lsp`; verification counter-examples are
   rendered inline when the server provides them.
5. Use the inline CodeLens actions above atoms and contracts to inspect intent
   drift and spec-code mapping.

### LSP smoke test

Use this snippet to confirm completion and definition support in a local
extension host:

```mumei
type Nat = i64 where v >= 0;

atom increment(n: Nat)
requires: n >= 0;
ensures: result >= 1;
body: { n + 1 };

atom double_increment(n: Nat)
requires: n >= 0;
ensures: result >= 1;
body: {
    let x = increment(n);
    increment(x)
};
```

Expected behavior:

- Completion includes Mumei keywords such as `atom`, `requires`, and `ensures`.
- Completion includes open-document atom names such as `increment`.
- Go to Definition on `increment(x)` jumps to `atom increment`.
- Syntax highlighting scopes keywords, contracts, built-in types, comments,
  strings, numbers, and operators.
- Comment toggling, bracket matching, auto-closing pairs, and brace folding use
  `language-configuration.json`.

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

## Known Issues

- The language server requires a working `mumei` binary. If it is not on
  `PATH`, set `mumei.serverPath` to an absolute path.
- Completion and definition results are based on open and parsed `.mm`
  documents; cross-workspace symbols may require opening the source file first.
- Diagnostics run the verifier and may depend on local Z3/LLVM setup for files
  that exercise verification-heavy features.

## Links

- [VS Code Marketplace](https://marketplace.visualstudio.com/items?itemName=mumei-lang.mumei)
- [Mumei Language](https://github.com/mumei-lang/mumei)
- [Issue Tracker](https://github.com/mumei-lang/mumei/issues)
- [Changelog](CHANGELOG.md)

## Publishing (maintainers)

The extension is packaged and (optionally) published by the
[`Publish VS Code Extension`](../../.github/workflows/publish-vscode.yml)
workflow. It runs automatically when a tag matching `vscode-v*` is pushed,
and can also be invoked manually from the Actions tab (`workflow_dispatch`).

### Release checklist

1. Confirm `package.json` has the next version and `publisher: "mumei-lang"`.
2. Update `CHANGELOG.md`.
3. Run:

   ```bash
   npm install
   npm test
   npm run compile
   npx vsce package --out mumei.vsix
   ```

4. Install `mumei.vsix` locally and smoke-test syntax highlighting, completion,
   definition, diagnostics, CodeLens, and language configuration behavior.
5. Publish either through the workflow (`workflow_dispatch` with
   `publish=true`, or a `vscode-v*` tag) or manually:

   ```bash
   npx vsce publish --packagePath mumei.vsix
   ```

6. Verify the Marketplace listing and install the published
   `mumei-lang.mumei` extension in a clean VS Code profile.

To actually push to the Marketplace, configure a repository secret named
`VSCE_PAT` containing an Azure DevOps PAT with the "Marketplace → Manage"
scope for the `mumei-lang` publisher. Without the secret the workflow
still produces a `.vsix` artifact (suitable for manual upload via
`Extensions: Install from VSIX…`) but skips `vsce publish`.

## License

Apache-2.0
