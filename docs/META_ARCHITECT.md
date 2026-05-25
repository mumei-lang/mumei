# Meta-Architect: Architectural Refactoring Agent

## Overview

The Meta-Architect analyzes architectural failures that cannot be resolved by local
LLM edits alone. It uses cross-specification reports to inspect atom dependencies,
contract mismatches, and circular dependencies, then proposes interface-level
refactorings.

## When It Triggers

- Oscillation detection: repeated verifier failures with the same signature
- Budget exhaustion: retry, token, solver-time, or action-class limits
- Circular dependencies in the cross-specification dependency graph
- Caller/callee contract conflicts across atom boundaries

## Analysis Inputs

- `cross_spec.json` from `mumei verify --cross-spec-verify`
- Retry history from the self-healing loop
- Atom `requires` and `ensures` clauses from the current source

## Refactoring Strategies

1. Relax preconditions when a caller guarantees less than the callee requires
2. Strengthen postconditions when a caller needs a stronger callee guarantee
3. Add validation functions at module boundaries
4. Split atoms by extracting an interface layer for cycles or highly coupled nodes

## MCP Tools

- `analyze_contract_conflicts(source_code)`: returns normalized conflicts,
  circular dependencies, the dependency graph, and cross-spec summary.
- `propose_interface_refactoring(source_code, retry_history=None)`: returns
  deterministic interface-level proposals for agents to apply or review.
