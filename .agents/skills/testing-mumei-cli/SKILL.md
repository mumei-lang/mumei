# Testing Mumei CLI

How to build, test, and E2E verify the Mumei language toolchain.

## Environment Setup

Mumei requires LLVM 17 and libclang. Set these before any cargo command:

```bash
export LLVM_SYS_170_PREFIX=/usr/lib/llvm-17
export LIBCLANG_PATH=/usr/lib/llvm-17/lib
```

If LLVM 17 is not installed:
```bash
sudo apt-get install -y llvm-17 llvm-17-dev libclang-17-dev clang-17
```

## Build & Unit Tests

```bash
cargo build --release
cargo test          # All tests (currently 49)
cargo clippy --all-targets -- -D warnings  # Lint check
```

Pre-commit hooks run `cargo clippy` and `cargo test` automatically on commit.

## E2E Testing: REPL (`mumei repl`)

The REPL is an interactive stdin-based CLI. Test by piping commands:

```bash
echo ':help
:load std/json.mm
:env
:quit' | ./target/release/mumei repl
```

Expected behavior:
- Banner: "Mumei REPL v{VERSION}" with command list
- `:help` — Lists available commands (:check, :verify, :load, :env, :quit)
- `:load std/json.mm` — Reports "Loaded N definition(s)"
- `:env` — Shows registered atoms (Verified + Trusted), types, structs, enums
- `:quit` — Prints "Goodbye!" and exits cleanly

The REPL auto-loads `std/prelude` on startup, so prelude atoms (prelude_is_none, prelude_is_ok, prelude_is_some) and types (Pair, Option, Result, List) are always present.

## E2E Testing: Doc Generation (`mumei doc`)

### HTML format (single file)
```bash
./target/release/mumei doc std/json.mm --output /tmp/mumei_docs --format html
```
Generates `index.html` + `json.html` in output dir. Open in browser to verify styled atom cards with verified/trusted badges.

### Markdown format (directory scan)
```bash
./target/release/mumei doc std/ --output /tmp/mumei_docs_md --format markdown
```
Generates one `.md` file per module found in the directory (recursively scans for .mm files).

### Verification
- HTML: Check that `index.html` lists modules and per-module pages show atom signatures
- Markdown: Check that generated .md files contain `# module_name`, `## Atoms`, and function signatures

## Common Issues

- **inkwell build failure**: If `inkwell` fails to compile from git, check that `Cargo.toml` uses the crates.io version (`inkwell = { version = "0.8.0", ... }`) rather than a git dependency
- **Z3 not found**: The `mumei verify` and `:verify` REPL command require Z3. Run `mumei setup` or install Z3 manually. However, `mumei repl` and `mumei doc` work without Z3.
- **aarch64-linux**: Pre-built binaries are only available for x86_64 Linux and x86_64/aarch64 macOS. Building from source on aarch64 Linux may require manual LLVM setup.

## Devin Secrets Needed

No secrets required for building/testing. The repo is public.
