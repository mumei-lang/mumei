# Mumei Emitter Plugin Guide

Mumei can load external code-generation emitters at runtime from dynamic libraries. A plugin implements `mumei_core::emitter::Emitter`, exports the expected ABI symbols, and is installed under `~/.mumei/emitters/<name>/`.

## Cargo template

```toml
[package]
name = "mumei-emit-noop"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
mumei-core = { git = "https://github.com/mumei-lang/mumei", package = "mumei-core" }
```

When developing against a local checkout, replace the dependency with:

```toml
mumei-core = { path = "../mumei/mumei-core" }
```

## Required exports

Every plugin must export:

- `mumei_emitter_abi_version() -> u32`
- `mumei_create_emitter() -> *mut (dyn Emitter + Send + Sync)`

The ABI version must match `mumei_core::emitter::EMITTER_ABI_VERSION`.

## No-op sample plugin

```rust
use mumei_core::emitter::{Artifact, Emitter, EMITTER_ABI_VERSION};
use mumei_core::hir::HirAtom;
use mumei_core::parser::ExternBlock;
use mumei_core::verification::{ModuleEnv, MumeiResult};
use std::path::Path;

struct NoopEmitter;

impl Emitter for NoopEmitter {
    fn emit(
        &self,
        _hir_atom: &HirAtom,
        _output_path: &Path,
        _module_env: &ModuleEnv,
        _extern_blocks: &[ExternBlock],
    ) -> MumeiResult<Vec<Artifact>> {
        Ok(Vec::new())
    }
}

#[no_mangle]
pub extern "C" fn mumei_emitter_abi_version() -> u32 {
    EMITTER_ABI_VERSION
}

#[no_mangle]
pub extern "C" fn mumei_create_emitter() -> *mut (dyn Emitter + Send + Sync) {
    Box::into_raw(Box::new(NoopEmitter))
}
```

## Build and install

Build the plugin:

```bash
cargo build --release
```

Install it into the emitter directory matching the `--emit` name:

```bash
mkdir -p ~/.mumei/emitters/noop

# Linux
cp target/release/libmumei_emit_noop.so ~/.mumei/emitters/noop/

# macOS
cp target/release/libmumei_emit_noop.dylib ~/.mumei/emitters/noop/

# Windows
copy target\release\mumei_emit_noop.dll %USERPROFILE%\.mumei\emitters\noop\
```

Run Mumei with the plugin:

```bash
mumei build src/main.mm --emit noop
```

For `--emit <name>`, Mumei searches:

- Linux: `~/.mumei/emitters/<name>/libmumei_emit_<name>.so`
- macOS: `~/.mumei/emitters/<name>/libmumei_emit_<name>.dylib`
- Windows: `~/.mumei/emitters/<name>/mumei_emit_<name>.dll`

## Runtime safety

The host keeps the dynamic library loaded while the emitter object is alive. `emit()` calls are wrapped in a panic catcher, so plugin panics become `MumeiError` diagnostics instead of unwinding through Mumei.
