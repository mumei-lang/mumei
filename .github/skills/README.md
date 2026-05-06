# Mumei Forge Agent Skills

Reusable, composable verification primitives for the Mumei compiler and `.mm` language. Each skill is a self-contained `SKILL.md` prompt that guides an LLM agent through the same CLI/MCP workflows used by developers.

## Skill Catalogue

| Skill | Status | Description |
|-------|--------|-------------|
| verify | implemented | Run Z3 verification for `.mm` source and parse JSON/semantic feedback |
| build | implemented | Verify and emit LLVM IR artifacts with `mumei build --emit llvm-ir` |
| diagnose | implemented | Interpret `failure_type`, `actions`, `semantic_feedback`, and `counter_example` |
| certify | implemented | Generate `.proof.json` certificates and validate them with `verify-cert` |
| catalog | implemented | List std catalog entries, analyze gaps, and visualize dependency graphs |

## Agent

A single orchestration agent composes these skills into end-to-end workflows:

| Agent | Role |
|-------|------|
| mumei-forge | Z3 proof checking, LLVM IR compilation, proof certificates, and std catalog/gap analysis |

## MCP Mapping

| Skill | MCP tool |
|-------|----------|
| verify | `validate_logic(source_code)` |
| build | `forge_blade(source_code, output_name)` |
| diagnose | `validate_logic(source_code)` plus `semantic_feedback` / `machine_readable` parsing |
| certify | CLI: `mumei verify --proof-cert`, `mumei verify-cert` |
| catalog | `list_std_catalog()`, `analyze_std_gaps()`, `visualize_std_graph(format)` |

## Usage

Run individual workflows directly:

```bash
mumei verify input.mm --json
mumei build input.mm -o katana --emit llvm-ir
mumei verify input.mm --proof-cert --output input.proof.json
mumei verify-cert input.proof.json input.mm
```

From the repository checkout:

```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo run -- verify input.mm --json
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 cargo run -- build input.mm -o katana --emit llvm-ir
```
