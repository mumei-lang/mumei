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

- Output includes `Mumei REPL v0.2.0 (JIT enabled)`.
- `:help` output includes `:load <file>`, `:type <expr>`, and `:verify <atom>`.
- Multiline `atom inc(...)` is buffered as one atom; output does not include `Undefined variable: requires`, `Undefined variable: ensures`, or `Undefined variable: body`.
- `inc` is verified on definition and via `:verify inc`; output includes `Verified: inc` at least twice and does not include `JIT compile warning for 'inc'`.
- `:type inc(5)` prints `: i64`.
- `inc(5)` prints exactly `= 6`.
- `:load std/json.mm` prints `Loaded 40 definition(s) from 'std/json.mm'`.
- `:eval json_parse("{}")` prints `= <positive integer>` and output does not include `Symbols not found`, `JIT compile error`, or `Execution error`.
- `:quit` prints `Goodbye!`.

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
