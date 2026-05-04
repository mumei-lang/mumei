# mumei Claude Code Guide

## Overview

mumei is a mathematical proof-driven programming language that verifies uncertain AI-generated code with formal proofs before treating it as a trusted asset: 「AIが生成した不確実なコードを数学的証明で検証する言語」. The toolchain parses `.mm` source, proves contracts with Z3, and can emit LLVM IR and structured verification artifacts.

## MCP server

This repository ships a FastMCP stdio server in `mcp_server.py`. Claude Code detects the project-level `.mcp.json` and starts the `mumei-forge` server automatically when MCP tools are enabled for the project.

Install the MCP SDK before using the server:

```bash
pip install "mcp[cli]>=1.0"
```

Use `/mcp` in Claude Code to inspect the server and approve tools. MCP tools are exposed to Claude Code with names like `mcp__mumei-forge__validate_logic`.

### Tools

| Tool | Use |
| --- | --- |
| `validate_logic(source_code)` | Run Z3 verification only. Use this repeatedly while repairing code. |
| `forge_blade(source_code, output_name)` | Verify code and generate LLVM IR artifacts. Use after validation succeeds. |
| `execute_mm(source_code, output_name, command)` | Run `build`, `verify`, or `check` on temporary `.mm` source. |
| `get_inferred_effects(source_code)` | Check required effects before generating or editing code. |
| `get_allowed_effects(project_dir)` | Inspect the current effect boundary for the project. |
| `set_allowed_effects(allowed, denied)` | Dynamically update allowed and denied effect boundaries. |
| `list_std_catalog()` | List verified std/ modules, types, structs, and atom signatures. |
| `analyze_std_gaps()` | Analyze missing std/ components and propose next implementation targets. |
| `visualize_std_graph(format)` | Render the std/ dependency graph as `mermaid` or `dot`. |
| `measure_std_health()` | Measure std/ proof-health metrics. |
| `get_proof_certificate(module_path)` | Retrieve a proof certificate for a std/ module. |
| `generate_doc(source_code, format)` | Generate structured docs for `.mm` source (`json`, `markdown`, or `html`). |

## Recommended workflow

1. Call `get_allowed_effects` to confirm the active effect boundary.
2. Call `list_std_catalog` to find reusable verified components.
3. Call `get_inferred_effects` before generating code that may require effects.
4. Write or edit the `.mm` code.
5. Call `validate_logic`; if verification fails, read the feedback, revise the code, and validate again.
6. Call `forge_blade` for the final verified build.

## Basic `.mm` syntax

```mumei
effect Log;

atom increment(n: i64) -> i64
  requires: n >= 0;
  ensures: result > n;
  effects: [];
  body: n + 1;
```

- `atom` defines the smallest verifiable function unit.
- `requires` declares preconditions callers must satisfy.
- `ensures` declares postconditions Z3 must prove for every execution path.
- `effects` declares side effects the atom is allowed to perform.

## Reading verification errors

MCP verification responses include agent-oriented diagnostics:

- `semantic_feedback`: human-readable explanation of the failed proof obligation and likely repair direction.
- `machine_readable`: structured JSON containing failure type, actions, related locations, data flow, and conflicting constraints.
- `counter_example`: concrete Z3 model values that violate a contract; use these to understand the failing path and strengthen either the code or the contract.

Prefer fixing the `.mm` logic or contracts and re-running `validate_logic` over jumping directly to `forge_blade`.
