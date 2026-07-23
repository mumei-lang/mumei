"""Tests for scripts/check_contract_vocabulary.py MCP docstring extraction."""
from __future__ import annotations

import sys
import textwrap
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))

from check_contract_vocabulary import (
    SYNC_DOCS,
    _check_doc_contradiction_type_aliases,
    _count_mcp_tool_decorators,
    _extract_mcp_docstrings_ast,
    main,
)


def test_ast_extracts_simple_tool():
    src = textwrap.dedent('''\
        @mcp.tool()
        def hello(x: int) -> str:
            """Say hello."""
            return "hi"
    ''')
    blocks = _extract_mcp_docstrings_ast(src)
    assert blocks == ["Say hello."]


def test_ast_extracts_tool_with_name_arg():
    src = textwrap.dedent('''\
        @mcp.tool(name="custom_name")
        def hello(x: int) -> str:
            """Say hello with custom name."""
            return "hi"
    ''')
    blocks = _extract_mcp_docstrings_ast(src)
    assert blocks == ["Say hello with custom name."]


def test_ast_extracts_multiline_signature():
    src = textwrap.dedent('''\
        @mcp.tool()
        def hello(
            x: int,
            y: str,
            z: list[int],
        ) -> str:
            """Multi-line sig tool."""
            return "hi"
    ''')
    blocks = _extract_mcp_docstrings_ast(src)
    assert blocks == ["Multi-line sig tool."]


def test_ast_extracts_with_blank_line_before_docstring():
    src = textwrap.dedent('''\
        @mcp.tool()
        def hello(x: int) -> str:

            """Blank line before docstring."""
            return "hi"
    ''')
    blocks = _extract_mcp_docstrings_ast(src)
    assert blocks == ["Blank line before docstring."]


def test_ast_extracts_multiple_decorators():
    src = textwrap.dedent('''\
        @mcp.tool()
        def a() -> str:
            """Tool A."""
            pass

        @mcp.tool(name="b_custom")
        def b(x: int) -> str:
            """Tool B with args."""
            pass

        @mcp.tool()
        async def c() -> str:
            """Async tool C."""
            pass
    ''')
    blocks = _extract_mcp_docstrings_ast(src)
    assert blocks == ["Tool A.", "Tool B with args.", "Async tool C."]


def test_count_mismatch_detected():
    src = textwrap.dedent('''\
        @mcp.tool()
        def a() -> str:
            """Tool A."""
            pass

        @mcp.tool()
        def b() -> str:
            pass
    ''')
    blocks = _extract_mcp_docstrings_ast(src)
    count = _count_mcp_tool_decorators(src)
    assert count == 2
    assert len(blocks) == 1  # b has no docstring
    assert count != len(blocks)


def test_decorator_count_matches_real_mcp_server():
    mcp_server = Path(__file__).resolve().parents[1] / "mcp_server.py"
    if not mcp_server.exists():
        return
    text = mcp_server.read_text(encoding="utf-8")
    count = _count_mcp_tool_decorators(text)
    blocks = _extract_mcp_docstrings_ast(text)
    assert count == len(blocks), (
        f"Extraction count mismatch: {len(blocks)} docstrings vs {count} decorators"
    )


def test_trusted_atoms_doc_is_in_sync_surface():
    """docs/TRUSTED_ATOMS.md must be part of the docs-sync vocabulary surface."""
    names = {p.name for p in SYNC_DOCS}
    assert "TRUSTED_ATOMS.md" in names


def test_doc_contradiction_type_alias_detected(tmp_path):
    doc = tmp_path / "DOC.md"
    doc.write_text(
        "The report exposes a `contradiction_kind` field.\n",
        encoding="utf-8",
    )
    violations = _check_doc_contradiction_type_aliases(doc)
    assert len(violations) == 1
    assert "contradiction_kind" in violations[0].message


def test_doc_contradiction_type_clean_doc_has_no_violation(tmp_path):
    doc = tmp_path / "DOC.md"
    doc.write_text(
        "The report exposes a `contradiction_type` field.\n",
        encoding="utf-8",
    )
    assert _check_doc_contradiction_type_aliases(doc) == []


def test_full_check_passes_on_real_docs():
    """The full vocabulary gate must pass on the checked-in docs surface."""
    assert main() == 0


def test_non_mcp_tool_decorators_ignored():
    src = textwrap.dedent('''\
        @app.route("/")
        def index():
            """Not an MCP tool."""
            pass

        @mcp.tool()
        def real_tool() -> str:
            """Real MCP tool."""
            pass
    ''')
    blocks = _extract_mcp_docstrings_ast(src)
    assert blocks == ["Real MCP tool."]
