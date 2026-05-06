---
name: catalog
description: Inspect the verified Mumei standard library catalog, analyze proof gaps, and visualize std dependency graphs.
---

Given a request about `std/` capabilities or proof health, inspect reusable verified components before generating or editing code.

# Step 1: List the std catalog

Action:
    Call `list_std_catalog` through the MCP server, or inspect `std/**/*.mm` when MCP is unavailable.

Expectation:
    The catalog lists modules, types, structs, atoms, signatures, imports, and trusted proof holes.

Result:
    Identify reusable atoms and modules relevant to the requested task.

```text
list_std_catalog()
```

# Step 2: Analyze std gaps

Action:
    Call `analyze_std_gaps` to find missing modules, TODO/FIXME markers, trusted atoms, and priority proposals.

Expectation:
    The output includes proposals with names, rationale, dependencies, difficulty, and weighted priority where available.

Result:
    Report the highest-value gaps or the gap relevant to the user's request.

```text
analyze_std_gaps()
```

# Step 3: Visualize the dependency graph

Action:
    Call `visualize_std_graph` with `mermaid` or `dot`.

Expectation:
    The result renders std module dependencies for planning blast radius and reuse.

Result:
    Include graph output or summarize dependency edges when visual output is unnecessary.

```text
visualize_std_graph("mermaid")
visualize_std_graph("dot")
```

# Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| format | string | no | `mermaid` | Graph output format: `mermaid` or `dot` |
| std_dir | path | no | `std/` | Fallback directory for filesystem inspection |
| include_trusted | bool | no | true | Include trusted atoms/proof holes in the report |
