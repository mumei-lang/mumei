# Mumei Visualizer

Two graph views over what `mumei verify` produces:

| View | Source | Rendering |
|---|---|---|
| Proof Graph (P26) | `proof_graph.json` (`mumei verify --emit proof-graph`) | interactive, `visualizer/app.py` |
| std/ dependency graph (SI-5 Phase 1-C) | `std/*.mm` | static Mermaid/DOT, `visualizer/generate_graph.py` |

Node colours are the same verification health in both: green = proven with no
trust boundary, yellow = trusted / proof-hole boundary, red = failed or
unverifiable.

## Proof Graph

### 1. Export `proof_graph.json`

`--emit proof-graph` implies cross-spec verification, so pass every file of the
project (entry point last, the rest via `--cross-spec-files`):

```bash
mumei verify --emit proof-graph --report-dir reports \
    --cross-spec-files tests/fixtures/session_types/payment_server.mm \
    tests/fixtures/session_types/payment_client.mm
```

This writes `reports/proof_graph.json` next to the existing
`reports/cross_spec.json`. The document folds together the atom dependency
graph, each atom's `requires`/`ensures`, its trust-boundary classification and
the session protocol violations it participates in — see
[`docs/REPORT_SCHEMA.md`](../docs/REPORT_SCHEMA.md) for the schema. The
`visualize_proof_graph` MCP tool returns the same document (`format="dot"` for
Graphviz source).

### 2. Explore it

```bash
pip install streamlit          # only dependency; graph rendering is built in
streamlit run visualizer/app.py -- --report-dir reports
```

Pick an atom in the sidebar (or click through its dependencies/dependents) to
see its contract, which atoms it depends on and which depend on it, the trust
boundaries it crosses with their rationale, contract mismatches on its calls,
and any session protocol violation it takes part in. Inconsistent calls are
drawn as red dashed edges; the selected atom gets a bold border.

The JSON-to-graph conversion lives in `visualizer/proof_graph_lib.py` and is
Streamlit-free, so it is unit tested directly (`tests/test_proof_graph_lib.py`).

## std/ dependency graph

```bash
python visualizer/generate_graph.py --format mermaid --output visualizer/std_graph.md
```

Committed output: [`std_graph.md`](std_graph.md). The same graph is available
interactively under the dashboard's "std/ Dependency Graph" view mode.
