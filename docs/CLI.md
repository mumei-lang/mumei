---
layout: default
title: "CLI Reference — Mumei"
description: "Mumei CLI commands for running, verifying, emitting LLVM IR or binaries, and executing proof-driven programs."
keywords: "mumei CLI, formal verification CLI, LLVM, Z3, mumei run"
---

# Mumei CLI

## `mumei run`

```bash
mumei run src/main.mm
mumei run src/main.mm --emit binary
mumei run src/main.mm --emit llvm-ir -o dist/app
```

`mumei run <file>` performs the native execution pipeline in one command:

1. parses and resolves the module, including imports and `mumei.toml` dependencies
2. verifies every atom with Z3
3. lowers all atoms into one LLVM module
4. exports `atom main()` as the C-compatible `main` entrypoint
5. compiles LLVM IR to an object file for `--emit binary`
6. links the object/IR with `clang` (falling back to `cc`/`gcc` for object files) and `runtime/mumei_runtime.c`
7. executes the resulting binary and returns its exit code

`atom main()` must be present and take no parameters. Its integer or floating-point result is converted to the process exit code. Runtime support includes channel helpers, named resource mutex lookup, and default effect-handler stubs for compiled `perform Effect.operation(...)` calls.
