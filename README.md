# Mumei (無銘) [![GitHub](https://img.shields.io/github/stars/mumei-lang/mumei?style=social)](https://github.com/mumei-lang/mumei)

**Verify existing code and specifications with formal methods — before you write `.mm`.**

Mumei is a formal verification toolchain that can start from existing foreign-language code (for example Python, Rust, Go, or TypeScript), natural-language requirements, or Mumei `.mm` modules. It uses Z3, proof certificates, and AI-agent workflows to find bugs, spec drift, and contradictions, then gives you a path to gradually move critical logic into mathematically checked `.mm` code.

[Technical Paper](paper/) — proof-driven programming architecture, autonomous verification loop, and case studies.

> existing code / natural language spec → MCP or mumei-agent → Z3-backed diagnostics → optional `.mm` migration → LLVM / proof artifacts

---

## Start without writing `.mm`

See the mumei-agent [Verification Workflow Guide](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/VERIFICATION_WORKFLOW_GUIDE.md) for the full no-`.mm` workflow, including natural-language spec validation, foreign-code verification, and spec↔code alignment.

### 1. Existing code: find likely bug locations

Give the agent an existing source file and ask it to extract contracts, verify them, and report suspicious paths. The supported language set depends on the workflow; current agent paths cover Python/Rust/Go for cross-validation and Python/TypeScript/Rust for foreign-code verification.

```bash
uv run python -m agent validate-code --input src/payment.py --language python
uv run python -m agent verify-foreign --file src/lib.rs --language rust
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

Compare requirements against an implementation and ask for mismatches before migrating anything to `.mm`.

```bash
uv run python -m agent validate-spec-to-code \
  --spec docs/requirements/payment.txt \
  --code src/payment.py \
  --language python
```

For reverse drift detection:

```bash
uv run python -m agent validate-code-to-spec \
  --code src/payment.py \
  --spec docs/requirements/payment.txt \
  --language python
```

### 3. Spec only: find contradictions and under-specified behavior

Start from prose requirements and check for direct contradictions, vacuity, ambiguity, and over-constraints.

```bash
uv run python -m agent validate-spec --input docs/requirements/payment.txt --format nl
uv run python -m agent extract-spec \
  --text-file docs/requirements/payment.txt \
  --check-contradiction-only \
  --output reports/payment_spec_report.json
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
uv run python -m agent validate-code --input src/payment.py --language python
uv run python -m agent validate-spec-to-code --spec spec.txt --code src/payment.py --language python
```

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
```

---

## Install

```bash
# One-liner (macOS / Linux)
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash

# Homebrew
brew install mumei-lang/mumei/mumei

# Specific version (latest is v0.6.0)
curl -fsSL https://mumei-lang.github.io/mumei/install.sh | bash -s -- --version v0.6.0
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

## Documentation

| Document | Content |
|----------|---------|
| [Verification Workflow Guide](https://github.com/mumei-lang/mumei-agent/blob/develop/docs/VERIFICATION_WORKFLOW_GUIDE.md) | No-`.mm` entry paths: natural-language specs, foreign-code verification, and spec↔code alignment |
| [MCP Integration](docs/MCP.md) | MCP tools, setup, and multi-agent collaboration |
| [Language Reference](docs/LANGUAGE.md) | Types, generics, traits, ownership, async |
| [Features](docs/FEATURES.md) | Feature matrix formerly summarized in this README |
| [Standard Library](docs/STDLIB.md) | Option, Result, List, BoundedArray, sort |
| [Examples & Tests](docs/EXAMPLES.md) | Verification suite, `.mm` code samples, and negative tests |
| [Architecture](docs/ARCHITECTURE.md) | Compiler internals and repository structure |
| [Report Schema](docs/REPORT_SCHEMA.md) | `report.json`, semantic feedback, and rich diagnostics JSON |
| [Cross-Spec Verification](docs/CROSS_SPEC_GUIDE.md) | System-wide contract consistency, invariants, and dependency cycles |
| [Toolchain](docs/TOOLCHAIN.md) | CLI commands, package management, CI/release |
| [LSP Integration](docs/LSP_INTEGRATION.md) | Editor CodeLens, intent drift, and spec-code mapping |
| [Roadmap](docs/ROADMAP.md) | Strategic roadmap |
| [Capability Security](docs/CAPABILITY_SECURITY.md) | Effect-based capability security evaluation |
| [Changelog](docs/CHANGELOG.md) | Release history |

---

## License

[Apache-2.0 license](LICENSE)
