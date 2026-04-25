"""Pure-Python helpers for inspecting the ``std/`` dependency graph.

These functions were previously private helpers inside ``mcp_server.py``
(the FastMCP tool server). They are pure-stdlib utilities (only ``re`` +
``pathlib``) that power both the ``visualize_std_graph`` MCP tool *and*
the ``visualizer/generate_graph.py`` CI script.

Exposing them as a standalone module (ws5 / SI-5 Phase 1-C follow-up)
means the graph-analysis / rendering code no longer depends on FastMCP
— so ``visualizer/generate_graph.py`` can import the helpers directly
without the lazy-import gymnastics it previously needed to avoid
executing ``mcp = FastMCP(...)`` at module import time.

Public API (intentionally small and stable):

* :func:`scan_std_imports(std_dir)`      — build dependency graph.
* :func:`collect_trusted_atoms(std_dir)` — one entry per ``trusted atom``.
* :func:`count_atoms_per_file(std_dir)`  — total atom count per file.
* :func:`trusted_by_file_counts(trusted_atoms)` — aggregate helper.
* :func:`classify_health(rel_path, trusted_by_file, failed_files)`
  — returns ``"green"`` / ``"yellow"`` / ``"red"``.
* :func:`sanitize_node_id(path)`         — Mermaid/DOT-safe identifier.
* :func:`render_std_graph_mermaid(...)`  — Mermaid ``graph TD`` source.
* :func:`render_std_graph_dot(...)`      — Graphviz DOT source.

Legacy underscore-prefixed aliases (``_scan_std_imports`` etc.) are kept
at the bottom of this module so existing ``from std_graph_lib import
_scan_std_imports`` call sites keep working. New code should prefer the
public names.
"""
from __future__ import annotations

import re
from pathlib import Path

__all__ = [
    "scan_std_imports",
    "collect_trusted_atoms",
    "count_atoms_per_file",
    "trusted_by_file_counts",
    "classify_health",
    "sanitize_node_id",
    "render_std_graph_mermaid",
    "render_std_graph_dot",
    # Legacy aliases re-exported for backwards compatibility:
    "_scan_std_imports",
    "_collect_trusted_atoms",
    "_count_atoms_per_file",
    "_trusted_by_file_counts",
    "_classify_health",
    "_sanitize_node_id",
    "_render_std_graph_mermaid",
    "_render_std_graph_dot",
]


def scan_std_imports(std_dir: Path) -> dict:
    """Scan all .mm files under std/ and build a dependency graph.

    Returns a dict mapping each ``std/*.mm`` file path to a sorted list of
    import targets (e.g. ``"std/prelude.mm"``). Files with no imports map
    to ``[]``. Only imports resolving to ``std/`` modules are included.
    """
    if not std_dir.exists():
        return {}

    available: dict = {}
    for mm_file in std_dir.rglob("*.mm"):
        rel = str(mm_file.relative_to(std_dir.parent)).replace("\\", "/")
        import_path = rel[: -len(".mm")]
        available[import_path] = rel

    dependency_graph: dict = {}
    # Accept both `import "std/xxx" as alias;` and `import "std/xxx";`
    import_re = re.compile(r'^\s*import\s+"([^"]+)"\s*(?:as\s+\w+\s*)?;')
    for mm_file in sorted(std_dir.rglob("*.mm")):
        rel = str(mm_file.relative_to(std_dir.parent)).replace("\\", "/")
        try:
            text = mm_file.read_text(encoding="utf-8")
        except OSError:
            dependency_graph[rel] = []
            continue
        deps: list = []
        for line in text.splitlines():
            m = import_re.match(line)
            if not m:
                continue
            target = m.group(1).strip()
            resolved = available.get(target)
            if resolved and resolved != rel and resolved not in deps:
                deps.append(resolved)
        dependency_graph[rel] = sorted(deps)
    return dependency_graph


def collect_trusted_atoms(std_dir: Path) -> list:
    """Scan for ``trusted atom <name>`` and return one entry per occurrence.

    Each entry contains ``file``, ``atom``, ``line`` number, and a short
    ``reason`` derived from the surrounding comments or body if available.
    """
    results: list = []
    trusted_re = re.compile(r"^\s*trusted\s+atom\s+(\w+)")
    for mm_file in sorted(std_dir.rglob("*.mm")):
        rel = str(mm_file.relative_to(std_dir.parent)).replace("\\", "/")
        try:
            lines = mm_file.read_text(encoding="utf-8").splitlines()
        except OSError:
            continue
        for idx, line in enumerate(lines):
            m = trusted_re.match(line)
            if not m:
                continue
            atom_name = m.group(1)
            # Heuristic reason extraction: grab the nearest preceding
            # comment block (// ...) or note a stub/empty body.
            reason = ""
            look = idx - 1
            while look >= 0 and lines[look].strip().startswith("//"):
                reason = lines[look].strip().lstrip("/ ").strip()
                look -= 1
            if not reason:
                # Peek up to 10 lines ahead for `body:` contents to detect stubs.
                end = min(idx + 10, len(lines))
                body_text = " ".join(l.strip() for l in lines[idx + 1 : end])
                if re.search(r"body\s*:\s*\{\s*\}", body_text):
                    reason = "body is stub"
                else:
                    reason = "trusted (reviewed contract)"
            results.append(
                {
                    "file": rel,
                    "atom": atom_name,
                    "line": idx + 1,
                    "reason": reason,
                },
            )
    return results


def count_atoms_per_file(std_dir: Path) -> dict:
    """Return ``{rel_path: total_atom_count}`` for every ``.mm`` file in std/."""
    atom_re = re.compile(r"^\s*(?:trusted\s+|async\s+)?atom\s+\w+")
    counts: dict = {}
    for mm_file in std_dir.rglob("*.mm"):
        rel = str(mm_file.relative_to(std_dir.parent)).replace("\\", "/")
        try:
            text = mm_file.read_text(encoding="utf-8")
        except OSError:
            counts[rel] = 0
            continue
        counts[rel] = sum(1 for line in text.splitlines() if atom_re.match(line))
    return counts


def trusted_by_file_counts(trusted_atoms: list) -> dict:
    """Aggregate :func:`collect_trusted_atoms` output as ``{rel_path: count}``."""
    counts: dict = {}
    for entry in trusted_atoms:
        rel = entry.get("file", "")
        if not rel:
            continue
        counts[rel] = counts.get(rel, 0) + 1
    return counts


def sanitize_node_id(path: str) -> str:
    """Return a Mermaid/DOT-safe identifier for a std file path."""
    return re.sub(r"[^A-Za-z0-9_]", "_", path)


def classify_health(
    rel_path: str,
    trusted_by_file: dict,
    failed_files: set,
) -> str:
    """Return one of ``"green"``, ``"yellow"``, ``"red"`` for a std file.

    * ``"red"``    — verification has failed or the file is explicitly marked broken.
    * ``"yellow"`` — at least one trusted atom (reviewed contract) present.
    * ``"green"``  — fully verified, no trusted atoms.
    """
    if rel_path in failed_files:
        return "red"
    if trusted_by_file.get(rel_path, 0) > 0:
        return "yellow"
    return "green"


def render_std_graph_mermaid(
    dependency_graph: dict,
    trusted_by_file: dict,
    atoms_by_file: dict,
    failed_files: set,
) -> str:
    """Render the std/ dependency graph as Mermaid ``graph TD`` source."""
    lines = ["graph TD"]
    # Node declarations: shape carries health semantics.
    #   green  -> rounded rectangle  id(label)
    #   yellow -> hexagon            id{{label}}
    #   red    -> doubled border     id[[label]]
    for path in sorted(dependency_graph.keys()):
        node_id = sanitize_node_id(path)
        atoms = atoms_by_file.get(path, 0)
        trusted = trusted_by_file.get(path, 0)
        label = f"{path}\\n{atoms} atoms, {trusted} trusted"
        health = classify_health(path, trusted_by_file, failed_files)
        if health == "green":
            lines.append(f'    {node_id}("{label}")')
        elif health == "yellow":
            lines.append(f'    {node_id}{{{{"{label}"}}}}')
        else:  # red
            lines.append(f'    {node_id}[["{label}"]]')

    # Edges.
    for path in sorted(dependency_graph.keys()):
        for dep in dependency_graph[path]:
            src = sanitize_node_id(path)
            dst = sanitize_node_id(dep)
            lines.append(f"    {src} --> {dst}")

    # Color classes.
    lines.append("    classDef green fill:#d4edda,stroke:#28a745,color:#155724;")
    lines.append("    classDef yellow fill:#fff3cd,stroke:#ffc107,color:#856404;")
    lines.append("    classDef red fill:#f8d7da,stroke:#dc3545,color:#721c24;")

    green_nodes, yellow_nodes, red_nodes = [], [], []
    for path in sorted(dependency_graph.keys()):
        node_id = sanitize_node_id(path)
        health = classify_health(path, trusted_by_file, failed_files)
        if health == "green":
            green_nodes.append(node_id)
        elif health == "yellow":
            yellow_nodes.append(node_id)
        else:
            red_nodes.append(node_id)
    if green_nodes:
        lines.append(f"    class {','.join(green_nodes)} green;")
    if yellow_nodes:
        lines.append(f"    class {','.join(yellow_nodes)} yellow;")
    if red_nodes:
        lines.append(f"    class {','.join(red_nodes)} red;")

    return "\n".join(lines) + "\n"


def render_std_graph_dot(
    dependency_graph: dict,
    trusted_by_file: dict,
    atoms_by_file: dict,
    failed_files: set,
) -> str:
    """Render the std/ dependency graph as Graphviz DOT source."""
    lines = [
        "digraph std_deps {",
        '    rankdir="TB";',
        "    node [shape=box, style=rounded];",
    ]
    for path in sorted(dependency_graph.keys()):
        node_id = sanitize_node_id(path)
        atoms = atoms_by_file.get(path, 0)
        trusted = trusted_by_file.get(path, 0)
        label = f"{path}\\n{atoms} atoms, {trusted} trusted"
        health = classify_health(path, trusted_by_file, failed_files)
        if health == "green":
            fill = "#d4edda"
            shape = "box"
            style = "rounded,filled"
        elif health == "yellow":
            fill = "#fff3cd"
            shape = "hexagon"
            style = "filled"
        else:
            fill = "#f8d7da"
            shape = "box"
            style = "rounded,filled,bold"
        lines.append(
            f'    {node_id} [label="{label}", shape={shape}, '
            f'style="{style}", fillcolor="{fill}"];',
        )
    for path in sorted(dependency_graph.keys()):
        for dep in dependency_graph[path]:
            src = sanitize_node_id(path)
            dst = sanitize_node_id(dep)
            lines.append(f"    {src} -> {dst};")
    lines.append("}")
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------
# Legacy underscore-prefixed aliases. These match the original private names
# used inside mcp_server.py so pre-existing call sites (including external
# tooling that imported them directly) keep working without edits.
# ---------------------------------------------------------------------------
_scan_std_imports = scan_std_imports
_collect_trusted_atoms = collect_trusted_atoms
_count_atoms_per_file = count_atoms_per_file
_trusted_by_file_counts = trusted_by_file_counts
_classify_health = classify_health
_sanitize_node_id = sanitize_node_id
_render_std_graph_mermaid = render_std_graph_mermaid
_render_std_graph_dot = render_std_graph_dot
