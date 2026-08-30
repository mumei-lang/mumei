"""Unit tests for the pure proof-graph conversion helpers (P26).

These run without Streamlit: the view layer only formats what
`visualizer.proof_graph_lib` returns.
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from visualizer.proof_graph_lib import (  # noqa: E402
    build_graph_elements,
    load_proof_graph,
    node_detail,
    render_proof_graph_dot,
    sanitize_node_id,
    summary_counts,
)


def _graph() -> dict:
    """A three-atom graph mirroring what `--emit proof-graph` writes."""
    return {
        "version": "1.0",
        "nodes": [
            {
                "atom_name": "client_send",
                "source_file": "order_client.mm",
                "requires": "order_id > 0",
                "ensures": "result == order_id",
                "effects": ["OrderChannel"],
                "dependencies": ["validate_order"],
                "dependents": [],
                "trust_boundaries": [
                    {
                        "kind": "effect_pre_override",
                        "rationale": "atom overrides the effect state machine's initial state",
                    }
                ],
                "verification_status": "verified",
                "health": "yellow",
                "session_protocol_violations": [0],
            },
            {
                "atom_name": "validate_order",
                "source_file": "order_protocol.mm",
                "requires": "order_id >= 0",
                "ensures": "result >= 0",
                "effects": [],
                "dependencies": [],
                "dependents": ["client_send"],
                "trust_boundaries": [],
                "verification_status": "verified",
                "health": "green",
                "session_protocol_violations": [],
            },
            {
                "atom_name": "server_reply",
                "source_file": "order_server.mm",
                "requires": "true",
                "ensures": "true",
                "effects": ["OrderChannel"],
                "dependencies": [],
                "dependents": [],
                "trust_boundaries": [
                    {"kind": "trusted_atom", "rationale": "atom is declared `trusted`"}
                ],
                "verification_status": "failed",
                "health": "red",
                "session_protocol_violations": [0],
            },
        ],
        "edges": [
            {
                "from": "client_send",
                "to": "validate_order",
                "is_consistent": False,
                "violations": ["caller does not guarantee callee requires"],
                "warnings": [],
            },
            {
                "from": "client_send",
                "to": "missing_atom",
                "is_consistent": True,
                "violations": [],
                "warnings": [],
            },
        ],
        "session_protocol_violations": [
            {
                "kind": "deadlock_no_progress",
                "effect": "OrderChannel",
                "caller_atom": "client_send",
                "callee_atom": "server_reply",
                "message": "both roles wait",
                "suggested_fix": "make one role send first",
            }
        ],
        "circular_dependencies": [["a", "b", "a"]],
        "summary": {},
    }


def test_sanitize_node_id_is_dot_safe() -> None:
    assert sanitize_node_id("Wallet::transfer") == "atom_Wallet_x3a__x3a_transfer"
    assert sanitize_node_id("plain_atom") == "atom_plain__atom"


def test_sanitize_node_id_keeps_similar_atom_names_distinct() -> None:
    names = [
        "Wallet::transfer",
        "Wallet__transfer",
        "Wallet_transfer",
        "Wallet.transfer",
    ]
    identifiers = [sanitize_node_id(name) for name in names]
    assert len(set(identifiers)) == len(names)
    for identifier in identifiers:
        assert re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", identifier)


def test_render_proof_graph_dot_escapes_atom_names() -> None:
    hostile = 'evil" ];  malicious [label="pwned'
    graph = {"nodes": [{"atom_name": hostile, "health": "green"}], "edges": []}
    dot = render_proof_graph_dot(graph)

    assert '\\" ];  malicious [label=\\"pwned' in dot
    # The graph attribute line plus one node statement: nothing was injected.
    assert len([line for line in dot.splitlines() if line.strip().endswith("];")]) == 2


def test_build_graph_elements_maps_nodes_and_drops_dangling_edges() -> None:
    elements = build_graph_elements(_graph())

    assert [node["atom_name"] for node in elements["nodes"]] == [
        "client_send",
        "validate_order",
        "server_reply",
    ]
    assert [node["health"] for node in elements["nodes"]] == ["yellow", "green", "red"]
    assert elements["nodes"][0]["trust_boundary_count"] == 1
    assert elements["nodes"][0]["session_protocol_violation_count"] == 1
    assert elements["nodes"][1]["style"]["fill"] == "#d4edda"
    assert elements["nodes"][2]["style"]["style"] == "rounded,filled,bold"

    # The edge to `missing_atom` has no node, so the viewer never draws it.
    assert [(edge["from"], edge["to"]) for edge in elements["edges"]] == [
        ("client_send", "validate_order")
    ]
    edge = elements["edges"][0]
    assert edge["source"] == "atom_client__send"
    assert edge["target"] == "atom_validate__order"
    assert edge["is_consistent"] is False
    assert edge["violations"] == ["caller does not guarantee callee requires"]


def test_unknown_health_is_greyed_out_rather_than_dropped() -> None:
    graph = {"nodes": [{"atom_name": "mystery", "health": "chartreuse"}], "edges": []}
    node = build_graph_elements(graph)["nodes"][0]
    assert node["health"] == ""
    assert node["style"]["fill"] == "#e2e3e5"


def test_render_proof_graph_dot_highlights_selection_and_mismatches() -> None:
    dot = render_proof_graph_dot(_graph(), selected_atom="client_send")

    assert dot.startswith("digraph proof_graph {")
    assert 'atom_client__send [label="client_send"' in dot
    assert "penwidth=4" in dot
    assert 'atom_validate__order [label="validate_order", shape=box' in dot
    assert 'fillcolor="#fff3cd"' in dot  # yellow trust-boundary node
    assert 'fillcolor="#f8d7da"' in dot  # red failed node
    assert 'atom_client__send -> atom_validate__order [color="#dc3545", style=dashed];' in dot
    assert "missing_atom" not in dot

    # Nothing is highlighted when no atom is selected.
    assert "penwidth=4" not in render_proof_graph_dot(_graph())


def test_node_detail_resolves_contracts_neighbours_and_violations() -> None:
    detail = node_detail(_graph(), "client_send")

    assert detail["requires"] == "order_id > 0"
    assert detail["ensures"] == "result == order_id"
    assert detail["effects"] == ["OrderChannel"]
    assert detail["dependencies"] == ["validate_order"]
    assert detail["dependents"] == []
    assert detail["trust_boundaries"][0]["kind"] == "effect_pre_override"
    assert detail["verification_status"] == "verified"
    assert detail["health"] == "yellow"

    # Violations are stored once at document level and referenced by index.
    assert [violation["kind"] for violation in detail["session_protocol_violations"]] == [
        "deadlock_no_progress"
    ]
    assert [edge["to"] for edge in detail["outgoing_edges"]] == ["validate_order", "missing_atom"]
    assert detail["incoming_edges"] == []

    reverse = node_detail(_graph(), "validate_order")
    assert reverse["dependents"] == ["client_send"]
    assert [edge["from"] for edge in reverse["incoming_edges"]] == ["client_send"]
    assert reverse["session_protocol_violations"] == []


def test_node_detail_ignores_out_of_range_violation_indices() -> None:
    graph = _graph()
    graph["nodes"][1]["session_protocol_violations"] = [7, -1]
    assert node_detail(graph, "validate_order")["session_protocol_violations"] == []


def test_node_detail_rejects_unknown_atoms() -> None:
    with pytest.raises(KeyError):
        node_detail(_graph(), "nope")


def test_summary_counts_are_recomputed_from_nodes() -> None:
    assert summary_counts(_graph()) == {
        "node_count": 3,
        "edge_count": 2,
        "green_count": 1,
        "yellow_count": 1,
        "red_count": 1,
        "trust_boundary_count": 2,
        "session_protocol_violation_count": 1,
        "circular_dependency_count": 1,
    }


def test_load_proof_graph_round_trips(tmp_path: Path) -> None:
    path = tmp_path / "proof_graph.json"
    path.write_text(json.dumps(_graph()), encoding="utf-8")
    assert load_proof_graph(path)["version"] == "1.0"


def test_load_proof_graph_rejects_foreign_documents(tmp_path: Path) -> None:
    path = tmp_path / "cross_spec.json"
    path.write_text(json.dumps({"contract_consistency": []}), encoding="utf-8")
    with pytest.raises(ValueError, match="not a proof graph"):
        load_proof_graph(path)

    malformed = tmp_path / "broken.json"
    malformed.write_text("{", encoding="utf-8")
    with pytest.raises(json.JSONDecodeError):
        load_proof_graph(malformed)

    with pytest.raises(FileNotFoundError):
        load_proof_graph(tmp_path / "absent.json")
