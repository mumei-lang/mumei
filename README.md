# Mumei (無銘) [![GitHub](https://img.shields.io/github/stars/mumei-lang/mumei?style=social)](https://github.com/mumei-lang/mumei)

[日本語版はこちら](README_JA.md)

**Verify existing code and specifications with formal methods — before you write `.mm`.**

Mumei is a formal verification toolchain that can start from existing code (for example Python, Rust, Go, or TypeScript), natural-language requirements, or Mumei `.mm` modules. It uses Z3, proof certificates, and AI-agent workflows to find bugs, spec drift, and contradictions, then gives you a path to gradually move critical logic into mathematically checked `.mm` code.

[Technical Paper](paper/) — proof-driven programming architecture, autonomous verification loop, and case studies.

> existing code / natural language spec → MCP or mumei-agent → Z3-backed diagnostics → optional `.mm` migration → LLVM / proof artifacts

## No-`.mm` front door and roadmap

The first user-facing route is the no-`.mm` audit path: run `mumei-agent audit --code-file ... --auto-migrate --auto-heal`, or use MCP `scan_and_fix`, before asking users to author `.mm` files.

For the detailed contract, canonical vocabulary, V1-A〜E order, Lean escalation rules, and PR evidence expectations, see [`docs/CROSS_PROJECT_ROADMAP.md`](docs/CROSS_PROJECT_ROADMAP.md), [`docs/ROADMAP.md`](docs/ROADMAP.md), and [`docs/ONBOARDING.md`](docs/ONBOARDING.md).

Recent standard-library sync points: `std/crypto/primitives.mm` is a forged,
Z3-decidable crypto predicate module that does not require Lean escalation;
mumei-lean live generated theorem coverage includes five paths —
`abs_saturating`, `bounded_mul_with_overflow_check`, `constant_time_eq_flag`,
`ff_zero_eq_zero`, and the sort ascending-preservation path
(`verified_insertion_sort_ascending`) which lowers `forall(i, 0, n-1, arr[i] <= arr[i+1])`
to mathlib's `List.Sorted` via the `MumeiLean.Sort` bridge.

---

## Start without writing .mm (mumei-agent)

mumei-agent lets you verify existing code and specifications as they are.
See the [Onboarding Guide](docs/ONBOARDING.md) for the path from no-`.mm` entry points to gradual `.mm` migration.

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
# --language is optional: python|rust|typescript|go (inferred from extension when omitted)
uv run mumei-agent validate-code --input src/payment.py
```

**2. Detect spec↔code drift**
```bash
# Optional: python|rust|typescript|go; omitted values are inferred from the extension
uv run mumei-agent validate-spec-to-code --spec docs/spec.txt --code src/payment.py \
  --language python
```

**3. Find contradictions in specs only**
```bash
# Optional; default: nl; other choices: human|json|markdown
uv run mumei-agent validate-spec --input docs/spec.txt --format nl
```

Domain hints such as `--domain financial` are optional. `validate-code --input`, `validate-spec-to-code --code`, and `validate-code-to-spec --code` take a single source file; directory recursion is available through `audit --code-file` and `extract-spec --code-file`.

Code validation and alignment commands (Layer B: Z3 strict verification) support `python|rust|typescript|go`. `extract-spec` (Layer A: spec extraction) auto-detects a broader set from file extensions: `rust`, `c`, `cpp`, `go`, `python`, `javascript`, `typescript`, and `java`.

See the [Verification Workflow Guide](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/VERIFICATION_WORKFLOW_GUIDE.md) for details.

## Self-Healing Loop: start without writing `.mm`

See the mumei-agent [Verification Workflow Guide](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/VERIFICATION_WORKFLOW_GUIDE.md) for the full no-`.mm` workflow, including natural-language spec validation, existing-code verification, and spec↔code alignment.
If you work from a source checkout of `mumei-agent`, run `uv sync` once; after that, the same commands are available as `uv run mumei-agent <subcommand>`.

### 1. Existing code: find likely bug locations

Give the agent an existing source file and ask it to infer contracts, verify them, and report suspicious paths. `--input` is required and points to a single source file. `--language` is optional (`python`, `rust`, `typescript`, or `go`); when omitted it is inferred from the file extension.

```bash
# --language is optional: inferred from extension when omitted
uv run mumei-agent validate-code --input src/payment.py
```

MCP agents can use mumei's verification backend directly once they synthesize or receive `.mm`:

```json
{
  "tool": "validate_logic",
  "arguments": {
    "source_code": "atom debit(balance: i64, amount: i64) requires: amount > 0; ensures: result >= 0; body: balance - amount;"
  }
}
```

### 2. Natural-language spec + existing code: detect spec↔code drift

Compare requirements against an implementation and ask for mismatches before migrating anything to `.mm`. `--spec` and `--code` are required; `--code` points to a single source file. `--language` is optional and can be `python`, `rust`, `typescript`, or `go`.

```bash
uv run mumei-agent validate-spec-to-code --spec docs/spec.txt --code src/payment.py
```

For reverse drift detection:

```bash
uv run mumei-agent validate-code-to-spec \
  --code src/payment.py \
  --spec docs/spec.txt \
  --language python  # Optional: python|rust|typescript|go
```

### 3. Spec only: find contradictions and under-specified behavior

Start from prose requirements and check for direct contradictions, vacuity, ambiguity, and over-constraints. `--input` is required; `--domain` is optional when you want domain-specific hints.

```bash
# --domain is optional
uv run mumei-agent validate-spec --input docs/spec.txt --domain payment  # --domain is optional
```

MCP agents can call spec-health or verification tools depending on whether the input is prose, extracted contracts, or `.mm`:

```json
{
  "tool": "forge_blade",
  "arguments": {
    "source_code": "atom safe_div(a: i64, b: i64) requires: b != 0; ensures: true; body: a / b;",
    "output_name": "safe_div"
  }
}
```

---

## Gradual migration path

### Step 0: Verify existing assets through MCP or mumei-agent

Run the agent on existing code and specs first. No `.mm` source is required.

```bash
mumei-agent audit --code-file src/payment.py --auto-migrate --auto-heal
mumei-agent validate-code --input src/payment.py
mumei-agent validate-spec-to-code --spec spec.txt --code src/payment.py
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

Mumei now participates in the four-repository NLAE pipeline as Module B (AR):
it reconstructs Mumei contracts into Z3 obligations, reports reconstruction
loss as structured JSON, and hands that Loss Vector to mumei-agent's
self-correction loop and mumei-lean's Fidelity Checker.

```text
mumei-agent (Module A / AV)
      ↓ generated .mm
mumei (Module B / AR)
      ↓ Loss Vector JSON
mumei-agent self-correct
      ↓ repaired certificate
mumei-lean Fidelity Checker
      ↓
mumei-demo Evaluation Loop
```

To inspect a verification failure as P9-E structured feedback:

```bash
mumei verify --emit loss-vector examples/nlae_integration_demo.mm
```

The output includes `status`, `error_type`, `location`,
`reconstruction_loss`, and `feedback_instruction`; agents can feed it into
`mumei-agent self-correct` or the `run_nlae_pipeline` MCP tool.

---

## Install

```bash
# One-liner (macOS / Linux)
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash

# Homebrew
brew install mumei-lang/mumei/mumei

# Specific version (latest is v0.6.10)
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash -s -- --version v0.6.10
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

## License

[Apache-2.0 license](LICENSE)
