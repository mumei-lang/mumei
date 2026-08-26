from pathlib import Path

from scripts import check_mcp_tool_contract


ROOT = Path(__file__).resolve().parents[1]


def test_mumei_tool_contract_matches_source() -> None:
    source = (ROOT / "mcp_server.py").read_text(encoding="utf-8")
    doc = (ROOT / "docs" / "MCP_TOOL_CONTRACT.md").read_text(encoding="utf-8")
    assert check_mcp_tool_contract.extract_tools(source) == (
        check_mcp_tool_contract.extract_documented_tools(doc)
    )


def test_sibling_agent_contract_is_validated(monkeypatch, tmp_path, capsys) -> None:
    agent_source = tmp_path / "agent" / "mcp_server.py"
    agent_source.parent.mkdir()
    agent_source.write_text(
        "@mcp.tool()\n"
        "def synthetic_agent_tool(value: str = 'default'):\n"
        "    return value\n",
        encoding="utf-8",
    )
    contract = tmp_path / "MCP_TOOL_CONTRACT.md"
    canonical = (ROOT / "docs" / "MCP_TOOL_CONTRACT.md").read_text(encoding="utf-8")
    start = canonical.index("## `mumei-agent`")
    end = canonical.find("\n## ", start + 1)
    agent_section = (
        "## `mumei-agent`\n\n"
        "| Tool | Arguments | Documented return keys |\n"
        "| --- | --- | --- |\n"
        "| `synthetic_agent_tool` | `value: str = 'default'` |  |\n"
    )
    contract.write_text(canonical[:start] + agent_section + canonical[end:], encoding="utf-8")
    monkeypatch.setattr(check_mcp_tool_contract, "SIBLING_AGENT_SERVER", agent_source)
    monkeypatch.setattr(check_mcp_tool_contract, "CONTRACT_DOC", contract)
    assert check_mcp_tool_contract.main() == 0
    output = capsys.readouterr().out
    assert "18 mumei-forge tools" in output


def test_sibling_agent_contract_absence_is_a_clean_skip(monkeypatch, tmp_path, capsys) -> None:
    missing = tmp_path / "mumei-agent" / "agent" / "mcp_server.py"
    monkeypatch.setattr(check_mcp_tool_contract, "SIBLING_AGENT_SERVER", missing)
    assert check_mcp_tool_contract.main() == 0
    output = capsys.readouterr().out
    assert "skipped" in output
    assert "sibling repository unavailable" in output
