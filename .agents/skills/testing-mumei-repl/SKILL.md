---
name: testing-mumei-repl
description: Test the Mumei JIT-backed REPL end-to-end. Use when changes touch `mumei repl`, REPL command handling in `src/main.rs`, JIT symbol mapping in `mumei-emit-llvm/src/jit.rs`, or `tests/test_repl.rs`.
---
# Testing Mumei REPL

## Devin Secrets Needed

None for local REPL/JIT/FFI CLI testing.

## Prerequisites

Build the CLI from the repo root and keep the LLVM/clang environment variables explicit:

```bash
cd /home/ubuntu/repos/mumei
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build
```

The binary is at `target/debug/mumei`. No browser recording is useful for this shell-only CLI flow; collect stdout/stderr logs instead.

## Primary REPL JIT/FFI Flow

Use this flow when validating stateful REPL behavior, multiline atom input, immediate JIT evaluation, or runtime FFI symbol resolution:

```bash
cd /home/ubuntu/repos/mumei
printf ':help\natom inc(x: i64) -> i64\n  requires: true;\n  ensures: result == x + 1;\n  body: { x + 1 }\n:type inc(5)\n:verify inc\ninc(5)\n:load std/json.mm\n:eval json_parse("{}")\n:quit\n' | \
  LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  target/debug/mumei repl
```

Expected assertions:

- Output includes `Mumei REPL` and `(JIT enabled)`.
- `:help` output includes `:load <file|dir>`, `:type <expr>`, `:verify <atom>`, `:verify-spec <path|inline>`, and `:verify-code <path>`.
- Multiline `atom inc(...)` is buffered as one atom; output does not include `Undefined variable: requires`, `Undefined variable: ensures`, or `Undefined variable: body`.
- `inc` is verified on definition and via `:verify inc`; output includes `Verified: inc` at least twice and does not include `JIT compile warning for 'inc'`.
- `:type inc(5)` prints `: i64`.
- `inc(5)` prints exactly `= 6`.
- `:load std/json.mm` prints `Loaded 40 definition(s) from 'std/json.mm'`.
- `:eval json_parse("{}")` prints `= <positive integer>` and output does not include `Symbols not found`, `JIT compile error`, or `Execution error`.
- `:quit` prints `Goodbye!`.

## No-.mm Agent Verification Flow

Use this flow when validating `:verify-spec` / `:verify-code` command handling. CI must not require a real `mumei-agent`; use a temporary fake `mumei-agent` on `PATH` for bucket-format assertions, and a separate empty `PATH` fixture to assert graceful missing-agent degradation.

Expected assertions for a fake-agent `:verify-spec` session:

- A contradictory spec prints `FAIL` and the fixed bucket names `spec_health_issues`, `verification_violations`, `cross_validation_gaps`, and `next_steps`.
- A healthy spec prints `PASS` with the same fixed bucket names.
- Piped/non-interactive stdin exits through `:quit`; it must not consume `:quit` as the answer to `修正しますか？ (y/n)`.
- With no `mumei-agent` on `PATH`, stderr includes `mumei-agent not found in PATH` and the REPL still prints `Goodbye!`.

The real CLI contract is:

```text
:verify-spec <path|inline>  -> mumei-agent validate-spec --input <tmp-or-file> --format json
:verify-code <path>         -> mumei-agent validate-code --input <file> (--language is optional: python|rust|typescript|go)
```

`validate-code` currently emits JSON by default and does not accept `--format json`; do not add that flag unless the mumei-agent CLI grows support for it.

## JIT Error Handling Flow

Use a deterministic compile-time error rather than relying on malformed arithmetic like `1 +`, because the parser may recover and evaluate it as `= 1`.

```bash
cd /home/ubuntu/repos/mumei
printf ':eval missing_symbol\n:quit\n' | \
  LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  target/debug/mumei repl
```

Expected assertions:

- Command exits zero after `:quit` rather than panicking or hanging.
- Output includes `JIT compile error: Codegen Error: Undefined variable: missing_symbol`.
- Output includes `Goodbye!`.
- Output does not include `thread 'main' panicked`, `segmentation fault`, or `stack backtrace`.

## Notes

- Run this from the repo root so `:load std/json.mm` resolves correctly.
- REPL verification prints detailed phase metrics; grep for the assertion strings above instead of relying on fixed line positions.
- If `json_parse("{}")` returns a different positive integer than a previous run, that is acceptable; it is an opaque runtime handle.
