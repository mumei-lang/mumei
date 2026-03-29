# Testing Mumei CLI

## Prerequisites

### Build
```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo build
```
Binary is at `target/debug/mumei`.

### Z3 Solver
Mumei requires the `z3` binary on PATH for verification commands (`verify`, `build`, `verify-cert`, `publish`).
```bash
sudo apt-get install -y z3
```
If Z3 is missing, commands exit immediately with "Z3 solver not found" — no partial output.

### Rust Toolchain
- Rust 1.94+ required
- LLVM 17 with Polly support (`/usr/lib/llvm-17`)

## Key CLI Commands for Testing

| Command | Purpose |
|---|---|
| `mumei verify <file.mm>` | Verify atoms with Z3 |
| `mumei verify --proof-cert <file.mm>` | Verify + generate `.proof.json` certificate |
| `mumei verify-cert <cert.json> <file.mm>` | Verify cert against current source |
| `mumei build <file.mm>` | Build (verify + compile to LLVM IR) |
| `mumei build --emit proof-cert <file.mm>` | Note: `--emit proof-cert` dispatches to `ProofCert` emit target but returns empty artifacts. Use `verify --proof-cert` instead for cert generation. |
| `mumei publish` | Publish current project to local registry (requires `mumei.toml`) |
| `mumei add <pkg>` | Add dependency from registry (requires `mumei.toml`) |
| `mumei verify --strict-imports <file.mm>` | Strict import mode |
| `mumei build --strict-imports <file.mm>` | Strict import mode for build |

## Test Files

- `sword_test.mm` — 8 atoms covering loops, floats, stack ops. Note: `scale` atom fails verification (float multiplication precision), so `all_verified` will be `false`.
- `examples/import_test/main.mm` — Imports `lib/math_utils.mm`, good for import/dependency testing.
- `examples/import_test/lib/math_utils.mm` — 2 simple atoms (`safe_add`, `safe_double`), all verify successfully. Good for generating clean certificates.
- `tests/test_cross_atom_chain.mm` — Effect system + chained atom composition.

## Testing Flows

### Certificate Generation + Verification (P5-A)
1. `mumei verify --proof-cert <file.mm>` generates `<stem>.proof.json`
2. `mumei verify-cert <cert.json> <file.mm>` checks cert against source
3. Modify source, re-run verify-cert to see "changed" status

### Publish + Add (P5-B)
1. Create project dir with `mumei.toml` (needs `[package]` with name/version and `[build]` with entry)
2. `mumei publish` from project dir
3. Check `~/.mumei/registry.json` for `cert_path`/`cert_hash`
4. Create consumer project, `mumei add <pkg-name>` to resolve from registry

### Project Setup for Publish Testing
```toml
# mumei.toml
[package]
name = "test-pkg"
version = "0.1.0"
description = "Test"

[build]
entry = "main.mm"
```

## Known Quirks

- **`--strict-imports` on direct file imports**: For `import "./path.mm"`, the resolver uses legacy behavior (trust atoms) when no cert exists, even with `--strict-imports`. Strict enforcement only applies at manifest dependency level.
- **`verify-cert` exit codes**: "changed" atoms produce exit 0 (soft warning). Only "unproven"/"missing" atoms cause exit 1.
- **Verification caching**: After first run, atoms are cached. Subsequent `verify` runs show "skipped (unchanged, cached)" for previously verified atoms. Delete `.mumei/` directory to clear cache.
- **`--emit proof-cert`**: The `build --emit proof-cert` flag is parsed but returns empty artifacts. Use `verify --proof-cert` for actual cert generation.

## Running Tests
```bash
# Full workspace tests
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo test --workspace

# Lint
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo clippy --workspace
cargo fmt --check
```

## Devin Secrets Needed
None required — all testing is local.
