---
name: testing-mumei-emitter-plugins
description: Test Mumei external emitter plugins end-to-end — building a plugin cdylib against mumei-core, installing it with `mumei add --emitter`, and proving `mumei build --emit <name>` dispatches to it. Use when changes touch `mumei add --emitter`, `cmd_add_emitter`, `mumei-core/src/emitter.rs` plugin loading/ABI, `EmitTarget::External`, or `tests/test_add_emitter.rs`.
---

# Testing Mumei emitter plugins

## Devin Secrets Needed

None. Everything is local CLI work.

## Prerequisites

```bash
cd /path/to/mumei
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build
```

Binary at `target/debug/mumei`. `z3` must be on PATH for any `build`/`verify` step.

## Always isolate the install root

Plugins install under `mumei_core::manifest::mumei_home()` = `dirs::home_dir()/.mumei`, so
`HOME=<tmpdir>` fully redirects installs and keeps the developer's real `~/.mumei` untouched.
Set `USERPROFILE` too if you care about Windows parity.

```bash
H=/tmp/mytest; rm -rf $H; mkdir -p $H
HOME=$H target/debug/mumei add --emitter smoke --path /tmp/mumei_emit_smoke
```

Installed layout: `$HOME/.mumei/emitters/<name>/libmumei_emit_<name>.so`
(`.dylib` on macOS; on Windows `mumei_emit_<name>.dll` with **no** `lib` prefix).

## Building a throwaway plugin that actually works

A plugin is a `cdylib` depending on `mumei-core` by path, exporting two `#[no_mangle] extern "C"`
symbols. Because the `Emitter` trait object is passed as a fat pointer split into
`EmitterPluginHandle { data, vtable }`, the plugin must be compiled by the same toolchain against
the same `mumei-core` source as the host binary — build it from a path dependency on the repo, not
from a published crate.

`Cargo.toml`:
```toml
[lib]
crate-type = ["cdylib"]
[dependencies]
mumei-core = { path = "/path/to/mumei/mumei-core" }
```

`src/lib.rs` (minimal but *observable* — emit a marker string so you can tell which build ran):
```rust
use mumei_core::emitter::{Artifact, ArtifactKind, Emitter, EmitterPluginHandle, EMITTER_ABI_VERSION};
use mumei_core::hir::HirAtom;
use mumei_core::parser::ExternBlock;
use mumei_core::verification::{ModuleEnv, MumeiResult};
use std::path::Path;

struct SmokeEmitter;
impl Emitter for SmokeEmitter {
    fn emit(&self, hir_atom: &HirAtom, output_path: &Path, _e: &ModuleEnv, _x: &[ExternBlock])
        -> MumeiResult<Vec<Artifact>> {
        Ok(vec![Artifact {
            name: output_path.with_extension("smoke.txt"),
            data: format!("SMOKE-PLUGIN-V1 atom={}\n", hir_atom.atom.name).into_bytes(),
            kind: ArtifactKind::Metadata,
        }])
    }
}
#[no_mangle] pub extern "C" fn mumei_emitter_abi_version() -> u32 { EMITTER_ABI_VERSION }
#[no_mangle] pub extern "C" fn mumei_create_emitter() -> EmitterPluginHandle {
    EmitterPluginHandle::from_boxed(Box::new(SmokeEmitter))
}
```

Verify the symbols before blaming the CLI: `nm -D --defined-only target/debug/libmumei_emit_smoke.so | grep mumei_`.

Note the debug `.so` is ~110 MB (it statically links `mumei-core`); build `--release` if size matters.

## Proving dispatch, not just installation

Installation success alone is weak evidence. Prove the *installed* library ran:

```bash
cp examples/import_test/lib/math_utils.mm $H/       # 2 atoms, verifies clean and fast
(cd $H && HOME=$H mumei build --emit smoke math_utils.mm)
cat $H/katana_safe_add.smoke.txt        # -> "SMOKE-PLUGIN-V1 atom=safe_add"
```

Artifacts are named `katana_<atom>.<ext>` (one per atom) and for `EmitTarget::External(_)` the
build writes everything the plugin returns regardless of `ArtifactKind`.

**Build two plugin generations** with different marker strings (`...-V1` vs `...-GEN2-V1`) when
testing `--force` replacement — otherwise a rename that silently does nothing looks identical to a
successful overwrite.

## Fixtures that make failure paths provable

| Fixture | How to build | Proves |
|---|---|---|
| ABI mismatch | `rustc --crate-type cdylib` of a file exporting only `mumei_emitter_abi_version() -> u32 { 9999 }` | `ABI version mismatch: expected 1, got 9999` |
| missing factory | same, but ABI returns `1` and no `mumei_create_emitter` | `does not export mumei_create_emitter` |
| not a library | `printf 'text' > libmumei_emit_x.so` | `Failed to load ...: file too short` |
| release-vs-debug | dir with `target/release/lib...so` = working plugin and `target/debug/lib...so` = garbage text | release precedence, because reversed order would *fail validation* instead of merely logging a different path |
| no library | empty directory | the "looked in ./, target/release/, target/debug/" message |

The release-vs-debug trick is the important one: asserting on the printed path alone would also pass
if precedence were broken but the log happened to be right.

## Assertions worth making for install atomicity

`cmd_add_emitter` stages to `<dest_dir>/.<libname>.incoming`, validates the staged path, then
`fs::rename`s onto the final name. To test that a failed install cannot damage a working one:

- `sha256sum` the installed library before and after a failing `--force`; it must be byte-identical.
- `find $HOME/.mumei -name '*incoming*'` must print nothing after **any** outcome.
- Re-run `mumei build --emit <name>` after the failure and check the marker string — the previous
  plugin must still load and run.
- A leftover `.<libname>.incoming` dotfile must not break anything: the loader resolves the exact
  `libmumei_emit_<name>.so` filename, so dropping garbage at the dotfile path should leave builds green.
- Failure messages: `The previous install at <path> is unchanged.` must appear only when a prior
  install exists — check a fresh HOME to confirm it is absent.
- Cross-name isolation: install names `alpha` and `beta`, fail a `--force` on `alpha`, and assert
  `beta`'s hash and build output are untouched.

Interleaving stdout and stderr in one capture makes the ordering look scrambled
(`🔌 Installing…` is stdout, errors are stderr). Redirect them to separate files before asserting on
message order, or you will chase a phantom bug.

## Command battery for this area

```bash
cargo test --test test_add_emitter
cargo test -p mumei-core --lib emitter
cargo fmt --check
cargo clippy --all-targets
python3 scripts/check_contract_vocabulary.py     # gates docs/CROSS_PROJECT_ROADMAP.md + src/cli.rs wording
```

`check_contract_vocabulary.py` inspects `src/cli.rs` help text and the roadmap docs, so changing an
`--emit`/`add` help string can fail it even when Rust compiles.

## Known sharp edges

- `resolve_plugin_source` accepts **any** existing file passed to `--path` without checking its
  filename, so `--path unrelated.so` installs it under the requested `<name>`. Rejection of bad
  candidates comes from the loader, not from filename matching.
- `--path` and `--force` are declared `requires = "emitter"`, but clap does not enforce that when a
  positional `<DEP>` is also given: `mumei add somedep --path /tmp --force` exits 0 and silently
  ignores both flags. If you are testing argument validation, assert on this case explicitly; it may
  still be unfixed.
- Unknown registry packages in dependency mode exit **0** with a warning and add `pkg = "*"` — that
  is long-standing `cmd_add` behaviour, not a regression.
