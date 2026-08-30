"""Pure helpers that turn ``proof_graph.json`` into graph elements (P26).

``mumei verify --emit proof-graph`` writes a single document that folds the
cross-spec dependency graph, each atom's ``requires``/``ensures``, the P23
trust-boundary classification and the session protocol violations together.
This module converts that document into the nodes/edges an interactive viewer
draws, and into the per-atom detail shown when a node is selected.

Everything here is pure stdlib and free of Streamlit, so the conversion is unit
testable without running the dashboard. The health palette matches
:mod:`std_graph_lib` so the interactive graph and the committed Mermaid/DOT
renderings agree on green/yellow/red.

Public API:

* :func:`load_proof_graph(path)`      — read and validate the document.
* :func:`build_graph_elements(graph)` — ``{"nodes": [...], "edges": [...]}``.
* :func:`render_proof_graph_dot(...)` — Graphviz DOT source.
* :func:`node_detail(graph, atom)`    — contracts, neighbours, violations.
* :func:`summary_counts(graph)`       — health/boundary/violation totals.
"""
from __future__ import annotations

import json
import re
from pathlib import Path

__all__ = [
    "HEALTH_STYLES",
    "PROOF_GRAPH_FILENAME",
    "build_graph_elements",
    "load_proof_graph",
    "node_detail",
    "render_proof_graph_dot",
    "sanitize_node_id",
    "summary_counts",
]

PROOF_GRAPH_FILENAME = "proof_graph.json"

# Same fills/shapes as `std_graph_lib.render_std_graph_dot`:
#   green  -> fully proven, rounded box
#   yellow -> trusted / proof-hole boundary, hexagon
#   red    -> failed or unverifiable, bold border
HEALTH_STYLES = {
    "green": {"fill": "#d4edda", "stroke": "#28a745", "shape": "box", "style": "rounded,filled"},
    "yellow": {"fill": "#fff3cd", "stroke": "#ffc107", "shape": "hexagon", "style": "filled"},
    "red": {"fill": "#f8d7da", "stroke": "#dc3545", "shape": "box", "style": "rounded,filled,bold"},
}
_UNKNOWN_HEALTH_STYLE = {
    "fill": "#e2e3e5",
    "stroke": "#6c757d",
    "shape": "box",
    "style": "rounded,filled",
}


def sanitize_node_id(atom_name: str) -> str:
    """Return a DOT-safe identifier for an atom name (``Foo::bar`` included)."""
    return "atom_" + re.sub(r"[^A-Za-z0-9_]", "_", atom_name)


def load_proof_graph(path) -> dict:
    """Load ``proof_graph.json`` from *path*.

    Raises ``FileNotFoundError`` when missing, ``json.JSONDecodeError`` when
    malformed and ``ValueError`` when the document is not a proof graph.
    """
    text = Path(path).read_text(encoding="utf-8")
    graph = json.loads(text)
    if not isinstance(graph, dict) or "nodes" not in graph or "edges" not in graph:
        raise ValueError(
            f"{path} is not a proof graph document "
            "(expected 'nodes' and 'edges'; run `mumei verify --emit proof-graph`)"
        )
    return graph


def _health(node: dict) -> str:
    health = node.get("health", "")
    return health if health in HEALTH_STYLES else ""


def health_style(health: str) -> dict:
    """Return the fill/stroke/shape for a health value, greying out unknowns."""
    return HEALTH_STYLES.get(health, _UNKNOWN_HEALTH_STYLE)


def build_graph_elements(graph: dict) -> dict:
    """Convert a proof graph document into drawable nodes and edges.

    Nodes carry the display label, health, style and the counts a viewer shows
    without opening the detail pane. Edges are dropped when either endpoint is
    absent from ``nodes[]``, so the rendered graph never dangles.
    """
    nodes = []
    known = set()
    for node in graph.get("nodes", []):
        atom_name = node.get("atom_name", "")
        if not atom_name:
            continue
        known.add(atom_name)
        health = _health(node)
        boundaries = node.get("trust_boundaries", []) or []
        nodes.append(
            {
                "id": sanitize_node_id(atom_name),
                "atom_name": atom_name,
                "label": atom_name,
                "health": health,
                "style": health_style(health),
                "source_file": node.get("source_file", "<unknown>"),
                "verification_status": node.get("verification_status"),
                "trust_boundary_count": len(boundaries),
                "session_protocol_violation_count": len(
                    node.get("session_protocol_violations", []) or []
                ),
            }
        )

    edges = []
    for edge in graph.get("edges", []):
        caller = edge.get("from", "")
        callee = edge.get("to", "")
        if caller not in known or callee not in known:
            continue
        edges.append(
            {
                "source": sanitize_node_id(caller),
                "target": sanitize_node_id(callee),
                "from": caller,
                "to": callee,
                "is_consistent": bool(edge.get("is_consistent", True)),
                "violations": list(edge.get("violations", []) or []),
                "warnings": list(edge.get("warnings", []) or []),
            }
        )

    return {"nodes": nodes, "edges": edges}


def render_proof_graph_dot(graph: dict, selected_atom: str = "") -> str:
    """Render the proof graph as Graphviz DOT, highlighting *selected_atom*.

    Inconsistent contract calls are drawn as red dashed edges so the pair that
    breaks a ``requires`` chain is visible without selecting either endpoint.
    """
    elements = build_graph_elements(graph)
    lines = [
        "digraph proof_graph {",
        '    rankdir="TB";',
        "    node [shape=box, style=rounded, fontname=\"Helvetica\"];",
    ]
    for node in elements["nodes"]:
        style = node["style"]
        attrs = [
            f'label="{node["label"]}"',
            f'shape={style["shape"]}',
            f'style="{style["style"]}"',
            f'fillcolor="{style["fill"]}"',
            f'color="{style["stroke"]}"',
        ]
        if node["atom_name"] == selected_atom:
            attrs.append('penwidth=4')
        lines.append(f"    {node['id']} [{', '.join(attrs)}];")
    for edge in elements["edges"]:
        attrs = []
        if not edge["is_consistent"]:
            attrs.append('color="#dc3545"')
            attrs.append('style=dashed')
        suffix = f" [{', '.join(attrs)}]" if attrs else ""
        lines.append(f"    {edge['source']} -> {edge['target']}{suffix};")
    lines.append("}")
    return "\n".join(lines) + "\n"


def node_detail(graph: dict, atom_name: str) -> dict:
    """Return everything the detail pane shows for *atom_name*.

    ``session_protocol_violations`` are resolved from the document-level list
    the node references by index, and ``incoming_edges``/``outgoing_edges``
    carry the contract-consistency verdict of each call.
    """
    node = next(
        (entry for entry in graph.get("nodes", []) if entry.get("atom_name") == atom_name),
        None,
    )
    if node is None:
        raise KeyError(f"atom '{atom_name}' is not in the proof graph")

    all_violations = graph.get("session_protocol_violations", []) or []
    violations = [
        all_violations[index]
        for index in node.get("session_protocol_violations", []) or []
        if isinstance(index, int) and 0 <= index < len(all_violations)
    ]
    edges = graph.get("edges", []) or []
    return {
        "atom_name": atom_name,
        "source_file": node.get("source_file", "<unknown>"),
        "requires": node.get("requires", "true"),
        "ensures": node.get("ensures", "true"),
        "effects": list(node.get("effects", []) or []),
        "health": _health(node),
        "verification_status": node.get("verification_status"),
        "dependencies": list(node.get("dependencies", []) or []),
        "dependents": list(node.get("dependents", []) or []),
        "trust_boundaries": list(node.get("trust_boundaries", []) or []),
        "session_protocol_violations": violations,
        "outgoing_edges": [edge for edge in edges if edge.get("from") == atom_name],
        "incoming_edges": [edge for edge in edges if edge.get("to") == atom_name],
    }


def summary_counts(graph: dict) -> dict:
    """Recompute the headline counts from ``nodes[]``/``edges[]``.

    The exporter already writes a ``summary`` block; recomputing keeps the view
    honest when it is handed a hand-edited or filtered document.
    """
    nodes = graph.get("nodes", []) or []
    counts = {
        "node_count": len(nodes),
        "edge_count": len(graph.get("edges", []) or []),
        "green_count": 0,
        "yellow_count": 0,
        "red_count": 0,
        "trust_boundary_count": 0,
        "session_protocol_violation_count": len(
            graph.get("session_protocol_violations", []) or []
        ),
        "circular_dependency_count": len(graph.get("circular_dependencies", []) or []),
    }
    for node in nodes:
        health = _health(node)
        if health:
            counts[f"{health}_count"] += 1
        counts["trust_boundary_count"] += len(node.get("trust_boundaries", []) or [])
    return counts
