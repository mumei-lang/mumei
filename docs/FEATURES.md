# Mumei Feature Matrix

| Category | Highlights |
|----------|------------|
| **Types** | Refinement types (`i64 where v >= 0`), structs, enums (ADT), generics, explicit return types (`-> Str`) |
| **Verification** | Pre/postconditions, [loop invariants + termination proof](LANGUAGE.md#termination-checking), `forall`/`exists` quantifiers, [temporal effect Z3 probes](ARCHITECTURE.md#stateful-effects-temporal-effect-verification), Lean translator contract metadata in proof certificates (`translator_version`, `binder_mapping`, `bridge_lemma_hash`) |
| **Traits** | [Algebraic laws verified by Z3](LANGUAGE.md#trait-definitions-with-laws) (`law reflexive: leq(x, x) == true`) |
| **Ownership** | [`ref` / `ref mut` / `consume`](LANGUAGE.md#ownership-and-borrowing) with Z3 aliasing prevention, MIR-based move analysis |
| **Concurrency** | `async`/`await`, `task_group:all`/`task_group:any`, [deadlock-free proof via resource hierarchy](LANGUAGE.md#asyncawait-and-resource-hierarchy) |
| **Effects** | Compile-time side-effect verification, `perform`/`effects:`, effect hierarchy, parameterized effects, [effect polymorphism (`<E: Effect>`)](LANGUAGE.md), [capability security](CAPABILITY_SECURITY.md), stateful effects with temporal ordering |
| **Lambda** | First-class closures `\|x, y\| x + y`, capture analysis |
| **Safety** | `trusted` / `unverified` atoms, taint analysis, BMC + inductive invariant, [`call_with_contract`](LANGUAGE.md#higher-order-functions-phase-a) for higher-order function verification |
| **FFI** | `extern "Rust"` / `extern "C"` blocks, handle-based memory management (`json_free`, `http_free`), `Str` type interop |
| **Std Library** | Option, Result, List, BoundedArray, Vector, HashMap, JSON, HTTP, sort algorithms, effect definitions |
| **Output** | LLVM IR (native binary), C header (`.h`) via `--emit c-header`, verified JSON metadata via `--emit verified-json`, Markdown proof certificates via `--emit proof-book`, Lean escalation bundles via `--proof-cert`, Rust / Python FFI bindings via `--emit rust` / `--emit python`, and runtime-loaded emitter plugins from `~/.mumei/emitters/<name>/` |
| **Emitter Architecture** | Cargo workspace emitter crates (`mumei-core`, `mumei-emit-llvm`, `mumei-emit-json`, `mumei-emit-proofbook`, `mumei-emit-rust`, `mumei-emit-python`); see [Plugin Guide](PLUGIN_GUIDE.md) and [Roadmap](CROSS_PROJECT_ROADMAP.md) |
| **Tooling** | LSP server, VS Code extension with counter-example ghost-text decorations, `mumei.toml` manifest, dependency manager, MCP server, contract-aware `mumei doc`, semantic feedback (bilingual EN/JP) |
