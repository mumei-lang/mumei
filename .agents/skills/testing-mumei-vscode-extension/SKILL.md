---
name: testing-mumei-vscode-extension
description: Test the Mumei VS Code extension release flow and LSP diagnostics end-to-end. Use when validating editors/vscode packaging, .mm activation, LSP completion/definition or certificate diagnostics, TextMate grammar, or language-configuration changes.
---

# Testing Mumei VS Code Extension

## Devin Secrets Needed

- `VSCE_PAT` — only needed for Marketplace publish/post-publish verification. If unavailable, explicitly mark `vsce publish` and Marketplace listing/install checks as untested.
- No secrets are needed for local VSIX packaging or isolated Extension Host validation.

## Scope

Use this skill for changes under `editors/vscode/`, especially `package.json`, `.vscodeignore`, `src/extension.ts`, `syntaxes/mumei.tmLanguage.json`, `language-configuration.json`, `CHANGELOG.md`, and `README.md`.

## Local prerequisites

- Build or verify that the local Mumei CLI exists at `target/debug/mumei`:
  ```bash
  LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build
  ./target/debug/mumei --version
  ```
- Install VS Code extension dependencies:
  ```bash
  (cd editors/vscode && npm install --no-package-lock)
  ```
- The VM may not have a normal `code` CLI; `code` can be aliased to Devin. Use `@vscode/test-electron` for automated Extension Host checks instead of assuming `code --install-extension` works.

## Package assertions

From `editors/vscode`, run:

```bash
npm test
npm run compile
npm run package -- --out mumei-0.3.0.vsix
```

Expected:

- `npm test` prints `VS Code extension assets validated`.
- `npm run compile` exits 0.
- `vsce package` exits 0 and prints `DONE  Packaged:`.
- The VSIX includes runtime dependencies required by `out/extension.js`, especially `node_modules/vscode-languageclient/` and transitive LSP packages (`vscode-jsonrpc`, `vscode-languageserver-protocol`, `vscode-languageserver-types`). If these are missing, the extension may package successfully but fail to activate.
- The VSIX excludes development-only files such as `src/**`, `tests/**`, `tsconfig.json`, `package-lock.json`, and TypeScript source files.

## Extension Host LSP assertions

Use `@vscode/test-electron` in a scratch directory to launch an isolated Extension Host with:

- `extensionDevelopmentPath`: `editors/vscode`
- `mumei.serverPath`: absolute path to `target/debug/mumei`
- workspace file: a `.mm` file containing at least one type and two atoms, for example:

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

Concrete assertions:

- Opening the file sets `document.languageId === "mumei"`.
- Extension `mumei-lang.mumei` is present and `isActive === true` after opening the `.mm` file.
- Disable VS Code word-based suggestions in the test host before checking completion, so completions must come from the LSP provider.
- Request completion at an untyped indentation/call-site position in the atom body. Completion labels should include `atom`, `requires`, `ensures`, `increment`, and `double_increment`.
- The `increment` completion should have VS Code API kind `CompletionItemKind.Function` and detail text containing `atom increment`, `requires: n >= 0`, and `ensures: result >= 1`; this distinguishes LSP completion from editor word suggestions.
- Request definition on the typed `increment(x)` identifier. It should resolve to the same test file at zero-based line `2` / one-based line `3`, where `atom increment(n: Nat)` is declared.

## Direct stdio certificate diagnostics

For server-only changes in `src/lsp.rs`, test the actual `target/debug/mumei lsp`
subprocess without installing VS Code. No secrets are required. Use
`tests/test_lsp_lean_escalation.rs` as the fixture and protocol reference.

- Generate a real sibling certificate with `mumei verify --proof-cert --output
  /tmp/fixture.proof.json /tmp/fixture.mm`; keep its original copy before patching
  certificate statuses to represent bridge results.
- Send a JSON-RPC `textDocument/didOpen` notification containing `uri` (an absolute
  `file://` URI), `languageId: "mumei"`, `version: 1`, and `text`. Frame UTF-8 bytes
  as `Content-Length: <byte count>\r\n\r\n<body>`. Close stdin and collect stdout
  and stderr with a timeout; parse response frames by byte length.
- Always assert a `textDocument/publishDiagnostics` notification for the exact
  URI, including negative checks. An empty/missing server response must not
  count as a successful absence of diagnostics.
- Certificate-derived diagnostics include `data.lean_escalation.certificate`.
  Check this field to distinguish them from live verification diagnostics.
  A `lean_verified` entry should produce source `mumei-lean`, severity `3`.
- Pending entries need an `escalation_reason` and a non-`lean_verified` result.
  Live-settled atoms suppress pending certificate entries, and bounded
  low-degree nonlinear atoms (every nonlinear variable with literal bounds,
  degree <= 2) are now nlsat-first and usually settle live. Use a fixture that
  stays outside that window, such as the reference's `symbolic_pow`
  (`requires: x >= 0;`, `ensures: result == x**y && result == x;`, `body: x;`),
  to test certificate-derived pending diagnostics without relying on a live
  failure. Removing the sibling certificate should remove that diagnostic.
  A `lean_verified` entry is only reported as verified (severity `3`) when its
  `lean_result_metadata` is current (status `lean_verified`, non-empty
  `theorem_name`, current `translator_version` / `bridge_lemma_hash`);
  otherwise it surfaces as `stale_translator` (severity `2`).
- Test edited buffer text independently of disk contents to catch accidental
  hashing of disk instead of the LSP buffer. Restore the buffer afterward as
  a positive control.
- For legacy compatibility tests, derive hash format and supported versions
  from `mumei-core/src/proof_cert/generation.rs`; do not simply relabel the
  certificate version while keeping a hash produced by a different algorithm.
- Preserve raw protocol frames, stderr, parsed diagnostics, and expected/actual
  assertions as shell-only evidence. Do not record an idle desktop.

## Grammar and language-configuration assertions

Load `editors/vscode/syntaxes/mumei.tmLanguage.json` and `editors/vscode/language-configuration.json` with Node and assert representative behavior:

- `atom-definition` matches `atom increment` and captures `atom` as `keyword.control.mumei` and `increment` as `entity.name.function.mumei`.
- `type-definition` matches `type Nat`.
- control keywords include `requires` and `ensures`.
- built-in type regex matches `Str`.
- operator regex matches `>=` and `->`.
- language config has line comment `//`, block comment `/* */`, `{}` bracket pair, double-quote auto-closing pair, brace indentation rules, and brace folding markers.

## Reporting notes

- For shell-only automated testing, do not record the desktop; attach logs and generated evidence screenshots instead.
- In PR comments, lead with untested Marketplace publish status if `VSCE_PAT` is unavailable.
- Mention any `vsce package` warning about bundling as a caution, not a failure, unless the acceptance criteria require bundling.
