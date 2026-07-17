# Mumei (無銘) [![GitHub](https://img.shields.io/github/stars/mumei-lang/mumei?style=social)](https://github.com/mumei-lang/mumei)

[日本語版はこちら](README_JA.md)

**Verify existing code and specifications with formal methods — before you write `.mm`.**

Mumei is a formal verification toolchain that can start from existing code (for example Python, Rust, Go, or TypeScript), natural-language requirements, or Mumei `.mm` modules. It uses Z3, proof certificates, and AI-agent workflows to find bugs, spec drift, and contradictions, then gives you a path to gradually move critical logic into mathematically checked `.mm` code.

[Technical Paper](paper/) — proof-driven programming architecture, autonomous verification loop, and case studies.

> existing code / natural language spec → MCP or mumei-agent → Z3-backed diagnostics → optional `.mm` migration → LLVM / proof artifacts

## No-`.mm` front door and roadmap

The first user-facing route is the no-`.mm` audit path: run `uv run mumei-agent audit --code-file ... --auto-migrate --auto-heal`, or use MCP `scan_and_fix`, before asking users to author `.mm` files.

For the detailed contract, canonical vocabulary, V1-A〜E order, Lean escalation rules, and PR evidence expectations, see [`docs/CROSS_PROJECT_ROADMAP.md`](docs/CROSS_PROJECT_ROADMAP.md), [`docs/ROADMAP.md`](docs/ROADMAP.md), and [`docs/ONBOARDING.md`](docs/ONBOARDING.md).

Recent standard-library and mumei-lean sync points are tracked in the
[Standard Library Reference](docs/STDLIB.md#cross-project-sync-points).

---

## Start without writing .mm (mumei-agent)

mumei-agent verifies existing code and specifications before you migrate
critical contracts to `.mm`. See the [Onboarding Guide](docs/ONBOARDING.md)
and the mumei-agent
[Verification Workflow Guide](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/VERIFICATION_WORKFLOW_GUIDE.md)
for supported languages, option details, MCP workflows, and gradual migration.

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

---

## Gradual migration path

### Step 0: Verify existing assets through MCP or mumei-agent

Run the agent on existing code and specs first. No `.mm` source is required.

```bash
uv run mumei-agent audit --code-file src/payment.py --auto-migrate --auto-heal
uv run mumei-agent validate-code --input src/payment.py
uv run mumei-agent validate-spec-to-code --spec spec.txt --code src/payment.py
```


MCP clients should use `scan_and_fix` for the same audit → migrate-suggest →
heal route. Cross-spec compiler artifacts use the same artifact vocabulary:
`cross_spec.json.contract_consistency[]` maps to agent `missing_constraints[]`,
`global_invariant_conflicts[]` maps to `divergences[]`, and
`circular_dependencies[]` maps to `drift_issues[]`.
Use the resulting counter-examples, drift reports, and suggested contracts as the migration backlog.

### Step 1: Start writing the critical spec surface in `.mm`

Convert the smallest high-risk contracts into `.mm` atoms and verify them with the CLI or MCP:

```bash
mumei verify specs/payment.mm
```

```json
{
  "tool": "validate_logic",
  "arguments": {
    "source_code": "atom transfer(balance: i64, amount: i64) requires: balance >= amount && amount > 0; ensures: result >= 0; body: balance - amount;"
  }
}
```

### Step 2: Write new verified code in `.mm`

Once the core contracts are stable, implement new logic directly in `.mm` and emit runnable artifacts.

```bash
mumei build src/main.mm -o dist/output
mumei run src/main.mm
mumei repl  # :verify-spec <path|inline>, :verify-code <path>, :verify <atom>
```

---

## P9 NLAE Integration

Mumei is Module B (AR) in the four-repository NLAE pipeline: it reconstructs
contracts into Z3 obligations and returns reconstruction loss as a structured
Loss Vector for self-correction and Lean fidelity checking.

```text
mumei-agent (Module A / AV)
      ↓ generated .mm
mumei (Module B / AR)
      ↓ Loss Vector JSON
uv run mumei-agent self-correct
      ↓ repaired certificate
mumei-lean Fidelity Checker
      ↓
mumei-demo Evaluation Loop
```

See [`docs/CROSS_PROJECT_ROADMAP.md` § P9](docs/CROSS_PROJECT_ROADMAP.md)
for phase status, artifacts, structured feedback fields, and the E2E workflow.

---

## Install

```bash
# One-liner (macOS / Linux)
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash

# Homebrew
brew install mumei-lang/mumei/mumei

# Specific version (latest is v0.6.14)
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash -s -- --version v0.6.14
```

See [Releases](https://github.com/mumei-lang/mumei/releases) for older versions and changelogs.

No Rust toolchain required. Detects OS/arch automatically.

<details>
<summary>Build from source</summary>

```bash
# macOS
brew install llvm@17 z3
# Linux
sudo apt-get install -y libz3-dev llvm-17-dev libclang-17-dev

cargo build --release   # -> target/release/mumei
cargo install --path .  # -> ~/.cargo/bin/mumei

# Or auto-install Z3/LLVM
mumei setup && source ~/.mumei/env
```

</details>

---

## Tooling reference

### CLI

| Command | Description |
|---------|-------------|
| `mumei build <file> -o <out>` | Verify + codegen (`--emit llvm-ir` (default) / `c-header` / `verified-json` / `proof-book` / `decidable-metrics` / `proof-cert` / `escalation-bundle` / `binary` / `rust` / `python` / external plugin name) |
| `mumei run <file>` | Verify → codegen → link → execute `atom main()` as a native binary (`--emit binary` default, `--emit llvm-ir` keeps IR before linking) |
| `mumei verify <file>` | Z3 verification only (`--emit loss-vector` prints P9-E structured feedback JSON) |
| `mumei check <file>` | Parse + resolve (fast, no Z3) |
| `mumei init <name>` | Generate project template |
| `mumei add <dep>` | Add dependency (path / git / registry) |
| `mumei publish` | Publish to local registry |
| `mumei list` | List available packages in the local registry |
| `mumei setup` | Download Z3 + LLVM toolchain |
| `mumei inspect` | Show development environment |
| `mumei infer-effects <file>` | Infer required effects (JSON output) |
| `mumei infer-contracts <file>` | Infer contracts for all atoms (JSON output) |
| `mumei repl` | Interactive REPL backed by ORC LLJIT — incremental atom compilation, cross-atom resolution, and atom redefinition; use `:verify-spec <path|inline>` / `:verify-code <path>` to validate natural-language specs and foreign code through `mumei-agent` |
| `mumei doc <file> -o <dir>` | Generate documentation (`--format html` (default) / `markdown` / `json`) |
| `mumei lsp` | Start LSP server; shows Z3 diagnostics, `/// spec:` natural-language spec health, and `.py` / `.rs` / `.ts` / `.tsx` / `.go` contract diagnostics from `mumei-agent` inline, with graceful fallback when the agent is unavailable |
| `mumei verify-cert <cert> <file>` | Verify a proof certificate against current source |

### MCP Tools

| Tool | Description |
|------|-------------|
| `forge_blade` | Verify + code generation in one step |
| `validate_logic` | Z3 verification only; returns counter-example and semantic feedback data |
| `execute_mm` | General-purpose build / check execution |
| `get_inferred_effects` | Pre-check: infer required effects before writing code |
| `get_allowed_effects` | Query current effect boundary for the session |
| `set_allowed_effects` | Override effect boundary dynamically |
| `analyze_std_gaps` | Identify gaps in std/ coverage |
| `list_std_catalog` | List all atoms in the std/ catalog |
| `visualize_std_graph` | Render std/ dependency graph (Mermaid or DOT) |
| `measure_std_health` | Measure std/ health metrics |
| `get_proof_certificate` | Retrieve proof certificate for a module |
| `generate_doc` | Generate structured documentation (`mumei doc --format json`) |
| `analyze_contract_conflicts` | Analyze cross-atom contract conflicts and circular dependencies (Meta-Architect) |
| `propose_interface_refactoring` | Propose interface-level refactorings for architectural issues (Meta-Architect) |
| `get_spec_guideline` / `get_spec_guidelines` | Return agent-facing spec-writing guidelines |
| `verify_with_orchestration` | Z3 verification with worker-pool orchestration, caching, and task tracking |
| `get_structured_feedback` | Return P9-E structured feedback JSON for source code |

### Project Structure

```text
mumei/
├── mumei-core/             # Core library: parser, HIR, verification, MIR, emitter trait
├── mumei-emit-llvm/        # LLVM IR emitter (LlvmEmitter + codegen)
├── mumei-emit-json/        # Verified JSON metadata emitter (VerifiedJsonEmitter)
├── mumei-emit-proofbook/   # Markdown proof-certificate emitter
├── mumei-emit-rust/        # Rust FFI binding emitter
├── mumei-emit-python/      # Python FFI binding emitter
├── mumei-ffi-tests/        # Generated Rust property tests for FFI contracts
├── src/                    # CLI binary (main.rs, cli.rs, lsp.rs, setup.rs)
├── std/                    # Standard library (.mm files)
├── runtime/                # C runtime library (mumei_runtime.c)
├── visualizer/             # std/ dependency graph generation scripts
├── scripts/                # Install script, utility scripts (install.sh, etc.)
├── benchmarks/             # Dafny-style and SV-COMP-style benchmarks
├── paper/                  # Technical paper
├── editors/vscode/         # VS Code extension (LSP client + counter-example decorations)
├── examples/               # Example programs
└── tests/                  # Integration tests (.mm files)
```

---

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

## Distributed Tracing (OpenTelemetry)

When built with `cargo build --features otel` and run with `OTEL_ENABLED=true`,
`mumei verify` exports spans via OTLP. This makes the full
`mumei-agent` (Python) → `mumei verify` (Rust) → Z3 path visible as one
distributed trace, so Jaeger or Grafana can identify which verification step
or Z3 solve is the bottleneck and show how much token usage and latency each
LLM call contributes.

If `TRACEPARENT` is set in the environment (W3C Trace Context), the Rust spans
become children of the caller's trace, preserving the end-to-end parent/child
relationship across the Python and Rust processes.

```bash
OTEL_ENABLED=true TRACEPARENT="00-..." mumei verify example.mm
```

OTel is **zero-cost when the feature is off**: with the default build,
`opentelemetry` is absent from the dependency tree. A malformed or absent
`TRACEPARENT` is ignored (verification still succeeds and produces identical
output), and `OTEL_ENABLED=true` degrades gracefully when no collector is
running. These invariants — plus the `mumei.verify.cli` → `mumei.z3.solve` span
parent/child relationship under an extracted `TRACEPARENT` — are enforced in CI
by [`.github/workflows/otel-tracing.yml`](.github/workflows/otel-tracing.yml)
and the `src/telemetry.rs` `otel` unit tests.

See [`docs/ROADMAP.md` § P15](docs/ROADMAP.md#p15-opentelemetry-分散トレース連携実装済み) for details.

---

## License

[Apache-2.0 license](LICENSE)
