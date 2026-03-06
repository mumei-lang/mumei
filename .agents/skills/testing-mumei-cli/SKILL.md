# Testing Mumei CLI

How to build, test, and E2E verify the Mumei language toolchain.

## Environment Setup

Mumei requires LLVM 17, libclang, and Z3. Set these before any cargo command:

```bash
export LLVM_SYS_170_PREFIX=/usr/lib/llvm-17
export LIBCLANG_PATH=/usr/lib/llvm-17/lib
```

If LLVM 17 or Z3 is not installed:
```bash
sudo apt-get install -y llvm-17 llvm-17-dev libclang-17-dev clang-17 z3
```

## Build & Unit Tests

```bash
cargo build
cargo test          # All tests (currently 57)
cargo clippy --all-targets -- -D warnings  # Lint check
```

Pre-commit hooks run `cargo fmt`, `cargo clippy`, and `cargo test` automatically on commit.
If `cargo fmt` modifies files during commit, stage the changes and retry the commit.

## E2E Testing: Verification (`mumei verify`)

Z3 must be installed (`z3 --version` must work). The `mumei verify` command runs Z3 formal verification.

### Testing successful verification
```bash
# Clear build cache to force fresh verification (otherwise results may be cached)
rm -f examples/.mumei_build_cache
./target/debug/mumei verify examples/higher_order_demo.mm
```
Expected: All atoms show "verified" with checkmark. Exit code 0.

### Testing miette rich error output
Create a file with a deliberate postcondition violation:
```bash
cat > /tmp/test_fail.mm << 'EOF'
atom bad(x: i64)
    requires: x >= 0;
    ensures: result == x + 2;
    body: x + 1;
EOF
./target/debug/mumei verify /tmp/test_fail.mm
```
Expected: Exit code 1. Rich error output with:
- Error code prefix (e.g. `mumei::verification`)
- Source code snippet with line numbers and underline markers
- Japanese help text (e.g. "ensures の条件を確認してください...")

If the output is plain text ("Verification Error: ...") without source context, miette integration may be broken.

## E2E Testing: Full Build (`mumei build`)

```bash
rm -f examples/.mumei_build_cache
./target/debug/mumei build examples/higher_order_demo.mm -o /tmp/ho_demo
```
Expected: Build completes with "Blade forged successfully". Generates:
- `/tmp/ho_demo.rs` (Rust transpilation)
- `/tmp/ho_demo.go` (Go transpilation)
- `/tmp/ho_demo.ts` (TypeScript transpilation)

Note: `.ll` (LLVM IR) file may be written to the current directory or a different location than `-o` path.

## E2E Testing: Parse Only (`mumei check`)

```bash
./target/debug/mumei check examples/higher_order_demo.mm
```
Expected: "Check passed: ... N atoms". Does NOT require Z3.

## E2E Testing: REPL (`mumei repl`)

The REPL is an interactive stdin-based CLI. Test by piping commands:

```bash
echo ':help
:load std/json.mm
:env
:quit' | ./target/debug/mumei repl
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
./target/debug/mumei doc std/json.mm --output /tmp/mumei_docs --format html
```
Generates `index.html` + `json.html` in output dir. Open in browser to verify styled atom cards with verified/trusted badges.

### Markdown format (directory scan)
```bash
./target/debug/mumei doc std/ --output /tmp/mumei_docs_md --format markdown
```
Generates one `.md` file per module found in the directory (recursively scans for .mm files).

## Build Cache

Mumei uses incremental build caching (`.mumei_build_cache` files) that stores atom hashes. When testing verification or build, **always clear the cache first** to ensure fresh results:
```bash
rm -f examples/.mumei_build_cache
rm -f /path/to/your/file/.mumei_build_cache
```
Otherwise you may see "skipped (unchanged, cached)" instead of actual verification.

## Common Issues

- **inkwell build failure**: If `inkwell` fails to compile from git, check that `Cargo.toml` uses the crates.io version (`inkwell = { version = "0.8.0", ... }`) rather than a git dependency
- **Z3 not found**: `mumei verify` and `mumei build` require Z3. Run `sudo apt-get install z3` or `mumei setup`. However, `mumei check`, `mumei repl`, and `mumei doc` work without Z3.
- **aarch64-linux**: Pre-built binaries are only available for x86_64 Linux and x86_64/aarch64 macOS. Building from source on aarch64 Linux may require manual LLVM setup.
- **cargo fmt on commit**: Pre-commit hooks may run `cargo fmt` and modify files. If commit fails with "cargo-fmt failed", stage the modified files and retry.
- **std library stubs**: Some std library atoms (e.g. `fold_left`, `list_map` in std/list.mm) are incomplete stubs marked `trusted`. They may fail at codegen/runtime. Do not test `mumei build` on them in isolation.

## Devin Secrets Needed

No secrets required for building/testing. The repo is public.
