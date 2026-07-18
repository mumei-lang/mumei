# Mumei (無銘) [![GitHub](https://img.shields.io/github/stars/mumei-lang/mumei?style=social)](https://github.com/mumei-lang/mumei)

[日本語版はこちら](README_JA.md)

**Verify existing code and specifications with formal methods — before you write `.mm`.**

Mumei is a formal verification toolchain that starts from existing code, natural-language requirements, or `.mm` modules. It uses Z3, proof certificates, and AI-agent workflows to find bugs, spec drift, and contradictions, then helps move critical logic into checked `.mm` code.

[Technical Paper](paper/) — proof-driven programming architecture, autonomous verification loop, and case studies.

> existing code / natural language spec → MCP or mumei-agent → Z3-backed diagnostics → optional `.mm` migration → LLVM / proof artifacts

## No-`.mm` front door and roadmap

Run `uv run mumei-agent audit --code-file ... --auto-migrate --auto-heal`, or MCP `scan_and_fix`, before asking users to author `.mm` files. See [`docs/CROSS_PROJECT_ROADMAP.md`](docs/CROSS_PROJECT_ROADMAP.md), [`docs/ROADMAP.md`](docs/ROADMAP.md), and [`docs/ONBOARDING.md`](docs/ONBOARDING.md) for the contract, vocabulary, V1-A〜E order, Lean escalation, and PR evidence.

Recent standard-library and mumei-lean sync points are tracked in the
[Standard Library Reference](docs/STDLIB.md#cross-project-sync-points).

## Start without writing .mm (mumei-agent)

mumei-agent verifies existing code and specifications before migration. See [`docs/ONBOARDING.md`](docs/ONBOARDING.md) and the [Verification Workflow Guide](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/VERIFICATION_WORKFLOW_GUIDE.md).

### Install

```bash
git clone https://github.com/mumei-lang/mumei-agent
cd mumei-agent
cp .env.example .env  # Set LLM_BASE_URL / LLM_API_KEY / LLM_MODEL
uv sync
# After this, run commands as uv run mumei-agent <subcommand>
```

### Three use cases

**1. Find likely bugs in existing code**

```bash
uv run mumei-agent validate-code --input src/payment.py
```

**2. Detect spec↔code drift**

```bash
uv run mumei-agent validate-spec-to-code --spec docs/spec.txt --code src/payment.py
```

**3. Find contradictions in specs only**

```bash
uv run mumei-agent validate-spec --input docs/spec.txt --format nl
```

## Gradual migration path

1. **Step 0:** Audit existing assets without `.mm`:
   `uv run mumei-agent audit --code-file src/payment.py --auto-migrate --auto-heal`
2. **Step 1:** Write a critical contract and verify it:
   `mumei verify specs/payment.mm`
3. **Step 2:** Implement new logic in `.mm` and emit artifacts:
   `mumei build src/main.mm -o dist/output`

MCP clients use `scan_and_fix` for the same audit → migrate-suggest → heal route. The cross-spec artifact vocabulary and migration guidance are in [`docs/CROSS_SPEC_GUIDE.md`](docs/CROSS_SPEC_GUIDE.md).

## P9 NLAE Integration

Mumei is Module B (AR) in the four-repository NLAE pipeline: it reconstructs contracts into Z3 obligations and returns a Loss Vector for self-correction and Lean fidelity checking.

```text
mumei-agent → generated .mm → mumei → Loss Vector JSON → self-correct → mumei-lean → mumei-demo
```

See [`docs/CROSS_PROJECT_ROADMAP.md`](docs/CROSS_PROJECT_ROADMAP.md) § P9 for phase status, artifacts, feedback fields, and E2E workflow.

## Distributed Tracing (OpenTelemetry)

With `cargo build --features otel` and `OTEL_ENABLED=true`, `mumei verify` exports OTLP spans and propagates `TRACEPARENT` across the mumei-agent → Rust → Z3 path. The feature is zero-cost when disabled and degrades gracefully without a collector. Details and CI coverage: [`docs/ROADMAP.md`](docs/ROADMAP.md) § P15 and [`.github/workflows/otel-tracing.yml`](.github/workflows/otel-tracing.yml).

## Install

```bash
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash
brew install mumei-lang/mumei/mumei
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash -s -- --version v0.6.14
```

See [Releases](https://github.com/mumei-lang/mumei/releases) for older versions. No Rust toolchain is required; OS/arch is detected automatically.

<details>
<summary>Build from source</summary>

```bash
brew install llvm@17 z3                 # macOS
sudo apt-get install -y libz3-dev llvm-17-dev libclang-17-dev  # Linux
cargo build --release
cargo install --path .
mumei setup && source ~/.mumei/env
```

</details>

## Tooling reference

The complete CLI command table is in [`docs/TOOLCHAIN.md`](docs/TOOLCHAIN.md), MCP tools and setup are in [`docs/MCP.md`](docs/MCP.md), and the full project structure is in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Documentation

| Document | Content |
|----------|---------|
| [Verification Workflow Guide](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/VERIFICATION_WORKFLOW_GUIDE.md) | Natural-language spec validation, existing-code verification, spec↔code alignment, and human-friendly operation guide |
| [MCP Integration](docs/MCP.md) | MCP tools, setup, and multi-agent collaboration |
| [Language Reference](docs/LANGUAGE.md) | Types, generics, traits, ownership, async |
| [Features](docs/FEATURES.md) | Feature matrix formerly summarized in this README |
| [Standard Library](docs/STDLIB.md) | Option, Result, List, BoundedArray, sort |
| [Examples & Tests](docs/EXAMPLES.md) | Verification suite, `.mm` code samples, and negative tests |
| [Architecture](docs/ARCHITECTURE.md) | Compiler internals and repository structure |
| [Report Schema](docs/REPORT_SCHEMA.md) | `report.json`, semantic feedback, and rich diagnostics JSON |
| [Cross-Spec Verification](docs/CROSS_SPEC_GUIDE.md) | System-wide contract consistency, invariants, and dependency cycles |
| [Toolchain](docs/TOOLCHAIN.md) | CLI commands, package management, CI/release |
| [Onboarding Guide](docs/ONBOARDING.md) | Gradual path from existing code and natural language to `.mm` |
| [LSP Integration](docs/LSP_INTEGRATION.md) | Editor CodeLens, intent drift, spec-code mapping, and `mumei-agent` spec/code diagnostics |
| [Roadmap](docs/ROADMAP.md) | Strategic roadmap |
| [Capability Security](docs/CAPABILITY_SECURITY.md) | Effect-based capability security evaluation |
| [Changelog](docs/CHANGELOG.md) | Release history |
| [Diagnostics](docs/DIAGNOSTICS.md) | Multi-span diagnostics, compound constraint decomposition |
| [Meta-Architect](docs/META_ARCHITECT.md) | Contract conflict analysis and interface refactoring tools |
| [Plugin Guide](docs/PLUGIN_GUIDE.md) | Emitter plugin development |
| [Proof Certificate](docs/PROOF_CERTIFICATE.md) | Proof certificate schema and usage |
| [Spec Guide](docs/SPEC_GUIDE.md) | Spec-writing guidelines for Z3-decidable fragments |
| [FFI](docs/FFI.md) | Foreign function interface (Rust/C) |
| [Concurrency](docs/CONCURRENCY.md) | Async/await and deadlock-free resource hierarchy |
| [Editors](docs/EDITORS.md) | VS Code and LSP editor integration |
| [Patterns](docs/PATTERNS.md) | Design patterns and idioms |
| [Trusted Atoms](docs/TRUSTED_ATOMS.md) | trusted/unverified atom usage |
| [Structured Feedback Schema](docs/STRUCTURED_FEEDBACK_SCHEMA.md) | P9-E structured feedback JSON schema |
| [Cross-Project Roadmap](docs/CROSS_PROJECT_ROADMAP.md) | mumei + mumei-agent ecosystem roadmap |
| [Claude Code Quickstart](docs/CLAUDE_CODE_QUICKSTART.md) | Quickstart guide for Claude Code users |

---

## License

[Apache-2.0 license](LICENSE)
