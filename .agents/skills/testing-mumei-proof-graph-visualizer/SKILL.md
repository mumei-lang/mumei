---
name: testing-mumei-proof-graph-visualizer
description: Test the Mumei proof-graph export (`mumei verify --emit proof-graph`) and the Streamlit dashboard in visualizer/app.py end-to-end. Use when changes touch mumei-core/src/proof_graph.rs, src/commands/verify.rs proof-graph emission, visualizer/proof_graph_lib.py, visualizer/app.py, or the visualize_proof_graph MCP tool.
---

# Testing the Mumei proof graph visualizer

## Export side (`mumei verify --emit proof-graph`)

Always run the CLI with the LLVM env:

```bash
export LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu
```

Two input forms, both worth exercising:

```bash
# explicit cross-spec list — NOTE: --cross-spec-files is comma-delimited
# (space-separated extra paths fail with "error: unexpected argument"); entry file last
cargo run -q -- verify --emit proof-graph --report-dir reports \
    --cross-spec-files dep_a.mm,dep_b.mm entry.mm

# directory form — should write exactly ONE project-wide proof_graph.json
cargo run -q -- verify --emit proof-graph --report-dir reports_dir path/to/dir/
```

Success line to look for: `🕸️  Proof graph written to: <dir>/proof_graph.json (N node(s), M edge(s))`.
`cross_spec.json` must still be written next to it. For the directory form, confirm the line appears
once and that `{n["source_file"] for n in nodes}` spans every file in the directory — a regression
here silently leaves only the last file's graph on disk.

Note the verify cache: atoms already verified print `skipped (unchanged, cached)` and still land in
the graph. If you need fresh statuses, use a fixture path that has never been verified (e.g. a copy
under `/tmp`).

## Fixtures that produce each colour / feature

Health colours are decided by `classify_health` in `mumei-core/src/proof_graph.rs`
(green = `verified`/absent status AND no trust boundary; red = `failed`/`unverifiable`;
everything else, including `unknown` and `escalation_candidate`, is yellow).

| what you want | how to get it |
|---|---|
| edge + **inconsistent** contract pair (red dashed edge) | `tests/test_cross_spec_multi_file_dep.mm` + `tests/test_cross_spec_multi_file.mm` — 7 nodes, 1 edge, `is_consistent: false` |
| yellow `effect_pre_override` + session protocol violation | `tests/fixtures/session_types/payment_server.mm` + `payment_client.mm` (deadlock_no_progress on `PaymentChannel`) |
| yellow `trusted_atom` | the `test_cross_spec_multi_file*` pair |
| red / `failed` | write a throwaway atom whose body cannot satisfy `ensures` (e.g. `ensures: result > x; body: { x }`) |
| yellow `escalation_candidate` | a nonlinear-arithmetic atom (`x * y * x`) verified with `--escalate-lean --warn-fragment`; `escalation_candidate` requires `emit_lean_artifacts` AND a fragment warning |

Most session-type fixtures (`payment_*`, `order_*`) yield **0 edges**, so they cannot prove the edge
rendering — use the `test_cross_spec_multi_file*` pair for anything edge-related.

## Dashboard side

`streamlit run visualizer/app.py -- --report-dir <dir>` — the bare `--` separator is required, and
the app also imports `std_graph_lib` from the repo root, so run it from the repo root. Streamlit is
the only UI dependency (`pip install --user streamlit`); Graphviz `dot` is **not** needed because
`st.graphviz_chart` renders client-side.

Testing tips:

- Start one instance per report dir on different ports (8501, 8502, ...) so scenarios can be compared
  by navigating rather than restarting servers. Launch with `setsid nohup ... &`: a plain background
  job dies when the exec shell exits, and `pkill -f "streamlit run"` can kill the launching shell too.
- Node colours/shapes and the selected-node bold border are only visible in pixels. Use the
  **Fullscreen** button Streamlit puts on hover at the top-right of the chart to get a screenshot
  where the edge style (red + dashed) and `penwidth=4` highlight are actually legible.
- Navigation is via the `→ callee` / `← caller` buttons in the detail pane; the sidebar `Atom`
  selectbox should follow along.
- Adversarial inputs to cover: empty report dir (`st.info` "No proof_graph.json found in ..."),
  truncated JSON (`st.error` with the JSONDecodeError position), and a valid-JSON-but-wrong document
  (copy `cross_spec.json` to `proof_graph.json` → "is not a proof graph document").
- `proof_graph_lib` is a pure JSON→DOT layer, so hand-crafting a `proof_graph.json` is a legitimate
  way to test hardening: atom names containing `"` / `]` / `;` verify DOT label escaping (they must
  render as ONE literal label, with no extra injected node), and the pair `a_b` / `a.b` verifies that
  `sanitize_node_id` is injective (they must stay two separate nodes, not merge into one).

## Known cosmetic issues (verify whether still present before reporting as new)

- Clicking a dependency/dependent navigation button can surface a Streamlit banner: *The widget with
  key "selected_atom" was created with a default value but also had its value set via the Session
  State API.* Caused by `app.py` writing `st.session_state["selected_atom"]` while the selectbox also
  passes `index=`/`key="selected_atom"`. Navigation still works.
- The Trust boundaries caption reads "— none (fully proven, zero-cost)" even for red (`failed`) and
  yellow (`escalation_candidate`) atoms, which have no boundary entries but are not proven.

## Devin Secrets Needed

None — everything runs locally.
