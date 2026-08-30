"""Mumei verification dashboard (P26).

Streamlit front-end for the artifacts `mumei verify` writes. The layout follows
the existing dashboard in `mumei-agent/visualizer/app.py` (page config, sidebar
view modes, `st.error`/`st.info` for missing or malformed inputs).

The "Proof Graph" view consumes `proof_graph.json`
(`mumei verify --emit proof-graph`) and lets you walk the dependency graph atom
by atom: selecting a node shows its `requires`/`ensures`, its dependencies and
dependents, the P23 trust boundaries it crosses and any session protocol
violation it participates in. All data-to-graph conversion lives in
`proof_graph_lib`, which is pure and unit tested without Streamlit.

Usage:
    mumei verify --emit proof-graph --report-dir reports app.mm
    streamlit run visualizer/app.py -- --report-dir reports
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import streamlit as st

REPO_ROOT = Path(__file__).resolve().parent.parent
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from std_graph_lib import (  # noqa: E402
    collect_trusted_atoms,
    count_atoms_per_file,
    render_std_graph_dot,
    scan_std_imports,
    trusted_by_file_counts,
)
from visualizer.proof_graph_lib import (  # noqa: E402
    PROOF_GRAPH_FILENAME,
    build_graph_elements,
    load_proof_graph,
    node_detail,
    render_proof_graph_dot,
    summary_counts,
)

HEALTH_LEGEND = {
    "green": "🟢 proven (no trust boundary)",
    "yellow": "🟡 trusted / proof-hole boundary",
    "red": "🔴 failed or unverifiable",
}


def _report_dir() -> Path:
    """Resolve `--report-dir` from the args after Streamlit's `--` separator."""
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--report-dir", default="reports")
    args, _ = parser.parse_known_args(sys.argv[1:])
    return Path(args.report_dir)


def _select_atom(atom_name: str) -> None:
    st.session_state["selected_atom"] = atom_name


def _render_proof_graph_view(report_dir: Path) -> None:
    graph_path = report_dir / PROOF_GRAPH_FILENAME
    if not graph_path.exists():
        fallback = Path(PROOF_GRAPH_FILENAME)
        if fallback.exists():
            graph_path = fallback
    if not graph_path.exists():
        st.info(
            f"No `{PROOF_GRAPH_FILENAME}` found in `{report_dir}`. "
            "Export it first:\n\n"
            "```bash\n"
            f"mumei verify --emit proof-graph --report-dir {report_dir} \\\n"
            "    --cross-spec-files other.mm app.mm\n"
            "```"
        )
        return

    try:
        graph = load_proof_graph(graph_path)
    except (OSError, json.JSONDecodeError, ValueError) as err:
        st.error(f"Failed to read {graph_path}: {err}")
        return

    counts = summary_counts(graph)
    st.caption(f"Source: `{graph_path}` (schema {graph.get('version', 'unknown')})")
    columns = st.columns(5)
    columns[0].metric("Atoms", counts["node_count"])
    columns[1].metric("Calls", counts["edge_count"])
    columns[2].metric("🟢 proven", counts["green_count"])
    columns[3].metric("🟡 trusted", counts["yellow_count"])
    columns[4].metric("🔴 failed", counts["red_count"])

    elements = build_graph_elements(graph)
    if not elements["nodes"]:
        st.info("The proof graph is empty — no atoms were analysed.")
        return

    atom_names = [node["atom_name"] for node in elements["nodes"]]
    if st.session_state.get("selected_atom") not in atom_names:
        st.session_state["selected_atom"] = atom_names[0]
    # The selection lives in session state so the navigation buttons can move
    # it; passing `index=` as well makes Streamlit warn on every rerun.
    selected = st.sidebar.selectbox("Atom", atom_names, key="selected_atom")
    st.sidebar.markdown("\n".join(f"- {label}" for label in HEALTH_LEGEND.values()))

    graph_column, detail_column = st.columns([3, 2])
    with graph_column:
        st.subheader("Dependency graph")
        st.graphviz_chart(render_proof_graph_dot(graph, selected), use_container_width=True)
        if counts["circular_dependency_count"]:
            st.warning(
                f"{counts['circular_dependency_count']} circular dependency chain(s): "
                + "; ".join(
                    " → ".join(cycle) for cycle in graph.get("circular_dependencies", [])
                )
            )

    detail = node_detail(graph, selected)
    with detail_column:
        st.subheader(f"{HEALTH_LEGEND.get(detail['health'], '⚪')[:2]} `{selected}`")
        st.caption(
            f"{detail['source_file']} · status: "
            f"{detail['verification_status'] or 'not verified in this run'}"
        )

        st.markdown("**Contract**")
        st.code(
            f"requires: {detail['requires']}\nensures:  {detail['ensures']}",
            language="text",
        )
        if detail["effects"]:
            st.markdown("**Effects**: " + ", ".join(f"`{effect}`" for effect in detail["effects"]))

        # Dependencies/dependents double as navigation, so a contract chain can
        # be walked without going back to the atom picker.
        st.markdown("**Depends on** (its `requires` must be discharged by the caller)")
        if detail["dependencies"]:
            for callee in detail["dependencies"]:
                st.button(f"→ {callee}", key=f"dep_{selected}_{callee}",
                          on_click=_select_atom, args=(callee,))
        else:
            st.caption("— none")

        st.markdown("**Depended on by**")
        if detail["dependents"]:
            for caller in detail["dependents"]:
                st.button(f"← {caller}", key=f"rdep_{selected}_{caller}",
                          on_click=_select_atom, args=(caller,))
        else:
            st.caption("— none")

        st.markdown("**Trust boundaries**")
        if detail["trust_boundaries"]:
            for boundary in detail["trust_boundaries"]:
                st.warning(f"`{boundary.get('kind', '')}` — {boundary.get('rationale', '')}")
        elif detail["health"] == "green":
            st.caption("— none (fully proven, zero-cost)")
        else:
            st.caption("— none")

        inconsistent = [
            edge
            for edge in detail["outgoing_edges"] + detail["incoming_edges"]
            if not edge.get("is_consistent", True)
        ]
        if inconsistent:
            st.markdown("**Contract mismatches**")
            for edge in inconsistent:
                st.error(
                    f"`{edge['from']}` → `{edge['to']}`: "
                    + "; ".join(edge.get("violations", []) or ["inconsistent call"])
                )

        st.markdown("**Session protocol violations**")
        if detail["session_protocol_violations"]:
            for violation in detail["session_protocol_violations"]:
                st.error(
                    f"`{violation.get('kind', '')}` on `{violation.get('effect', '')}`: "
                    f"{violation.get('message', '')}\n\n"
                    f"Suggested fix: {violation.get('suggested_fix', '')}"
                )
        else:
            st.caption("— none")

    with st.expander("Raw proof_graph.json"):
        st.json(graph)


def _render_std_graph_view() -> None:
    std_dir = REPO_ROOT / "std"
    if not std_dir.exists():
        st.info(f"No `std/` directory found under {REPO_ROOT}.")
        return
    dependency_graph = scan_std_imports(std_dir)
    trusted_atoms = collect_trusted_atoms(std_dir)
    trusted_by_file = trusted_by_file_counts(trusted_atoms)
    atoms_by_file = count_atoms_per_file(std_dir)
    st.subheader("std/ dependency graph")
    st.caption("Same health colours as the committed Mermaid rendering in visualizer/std_graph.md")
    st.graphviz_chart(
        render_std_graph_dot(dependency_graph, trusted_by_file, atoms_by_file, set()),
        use_container_width=True,
    )


def main() -> None:
    st.set_page_config(page_title="Mumei Proof Graph", page_icon="🗡️", layout="wide")
    st.title("🗡️ Mumei Verification Dashboard")

    view_mode = st.sidebar.radio(
        "View Mode",
        ["Proof Graph", "std/ Dependency Graph"],
        index=0,
    )
    if view_mode == "Proof Graph":
        _render_proof_graph_view(_report_dir())
    else:
        _render_std_graph_view()


main()
