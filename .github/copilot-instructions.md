# Mumei Compiler Development Guide

Always reference these instructions first and fall back to search or shell commands only when you encounter unexpected information that does not match the info here.

## Working Effectively

### Bootstrap and Build the Repository

Mumei is a Rust compiler/CLI for the `.mm` language. The compiler itself does **not** call LLMs; it verifies contracts with Z3 and emits LLVM IR or structured proof artifacts. LLM-driven workflows live in `mumei-lang/mumei-agent`.

Validated local setup:

```bash
sudo apt-get install -y libz3-dev z3 llvm-17-dev libclang-17-dev
export LLVM_SYS_170_PREFIX=/usr/lib/llvm-17
cargo build
```

Useful variants:

```bash
cargo build --release
cargo install --path .
mumei setup && source ~/.mumei/env
```

### Test and Validate

Run the fastest checks first:

```bash
cargo fmt --all -- --check
cargo build
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo run -- verify std/contracts.mm
python -m pytest tests/test_mcp_server.py -v
```

When touching Rust verification or CLI behavior, also run the relevant cargo tests:

```bash
cargo test
```

## CLI Reference

Common commands:

| Command | Purpose |
| --- | --- |
| `mumei build <file> -o <out>` | Verify + compile. Default emit target is LLVM IR. |
| `mumei build <file> --emit llvm-ir` | Emit LLVM IR artifacts after verification. |
| `mumei build <file> --emit c-header` | Emit C header for FFI. |
| `mumei build <file> --emit verified-json` | Emit structured verified artifact JSON. |
| `mumei build <file> --emit proof-book` | Emit proof-book output. |
| `mumei build <file> --emit proof-cert` | Emit a `.proof-cert.json` certificate. |
| `mumei verify <file>` | Run Z3 verification only. |
| `mumei verify <file> --json` | Print machine-readable verification report. |
| `mumei verify <file> --proof-cert --output <path>` | Generate a proof certificate. |
| `mumei verify-cert <cert> <file>` | Re-check a certificate against current source. |
| `mumei check <file>` | Parse, resolve, and monomorphize without Z3. |
| `mumei init <name>` | Generate a project template. |
| `mumei add <dep>` | Add a path, git, or registry dependency. |
| `mumei publish` | Publish to the local registry. |
| `mumei inspect --ai --format json` | Inspect toolchain and project state for agents. |
| `mumei infer-effects <file>` | Infer required effects as JSON. |
| `mumei infer-contracts <file>` | Infer requires/ensures suggestions as JSON. |
| `mumei doc <file> -o <dir>` | Generate HTML, Markdown, or JSON documentation. |
| `mumei lsp` | Start the Language Server Protocol server. |
| `mumei repl` | Start the interactive REPL. |

## Basic `.mm` Syntax

```mumei
type Nat = i64 where v >= 0;

effect Log;

atom increment(n: Nat) -> i64
  requires: n >= 0;
  ensures: result > n;
  effects: [];
  body: n + 1;
```

Key concepts:

- `atom` is the smallest verifiable function unit.
- `requires` declares caller obligations.
- `ensures` declares postconditions Z3 must prove for every path.
- `effects` declares the only side effects an atom may perform.
- `trusted atom` bypasses body verification and should be treated as a proof gap.
- `type T = base where predicate;` defines a refinement type.

## MCP Server

The repository ships a FastMCP stdio server in `mcp_server.py`. Install MCP support before use:

```bash
pip install "mcp[cli]>=1.0"
```

Project MCP configuration is in `.mcp.json`:

```json
{
  "mcpServers": {
    "mumei-forge": {
      "command": "sh",
      "args": ["-lc", "cd . && exec python mcp_server.py"]
    }
  }
}
```

Important MCP tools:

| Tool | Use |
| --- | --- |
| `validate_logic(source_code)` | Run Z3 verification only. Equivalent to the verify skill. |
| `forge_blade(source_code, output_name)` | Verify and emit LLVM IR artifacts. Equivalent to the build skill. |
| `execute_mm(source_code, output_name, command)` | Run `check`, `verify`, or `build` on temporary source. |
| `get_inferred_effects(source_code)` | Inspect effect requirements before editing/generating code. |
| `list_std_catalog()` | List std modules, types, structs, and atom signatures. |
| `analyze_std_gaps()` | Propose missing std components and proof-gap candidates. |
| `visualize_std_graph(format)` | Render the std dependency graph as `mermaid` or `dot`. |
| `measure_std_health()` | Measure std proof-health metrics. |
| `get_proof_certificate(module_path)` | Retrieve a std module proof certificate. |
| `generate_doc(source_code, format)` | Generate JSON, Markdown, or HTML docs for `.mm` source. |

## Verification Error Reports

Prefer `mumei verify --json` or `validate_logic` while repairing code. Agent-oriented fields include:

- `semantic_feedback`: human-readable explanation of violated constraints.
- `machine_readable`: structured diagnostics with `failure_type`, `actions`, related locations, data flow, and conflicting constraints.
- `counter_example`: concrete model values that violate a contract.
- `failure_type`: categories such as `postcondition_violated`, `precondition_violated`, `division_by_zero`, `linearity_violated`, `exhaustiveness_failed`, and `effect_not_allowed`.

Fix the `.mm` body or contracts and re-run verification before building.
