from pathlib import Path

from scripts.check_mcp_tool_contract import (
    extract_documented_tools,
    extract_tools,
)


ROOT = Path(__file__).resolve().parents[1]


def test_mumei_tool_contract_matches_source() -> None:
    source = (ROOT / "mcp_server.py").read_text(encoding="utf-8")
    doc = (ROOT / "docs" / "MCP_TOOL_CONTRACT.md").read_text(encoding="utf-8")
    assert extract_tools(source) == extract_documented_tools(doc)
