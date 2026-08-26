from pathlib import Path

from scripts.check_mcp_tool_contract import (
    SIBLING_AGENT_SERVER,
    extract_documented_tools,
    extract_tools,
    main,
)


ROOT = Path(__file__).resolve().parents[1]


def test_mumei_tool_contract_matches_source() -> None:
    source = (ROOT / "mcp_server.py").read_text(encoding="utf-8")
    doc = (ROOT / "docs" / "MCP_TOOL_CONTRACT.md").read_text(encoding="utf-8")
    assert extract_tools(source) == extract_documented_tools(doc)


def test_sibling_agent_contract_is_validated(capsys) -> None:
    assert SIBLING_AGENT_SERVER.is_file()
    assert main() == 0
    output = capsys.readouterr().out
    assert "mumei-agent validation" not in output
    assert "18 mumei-forge tools" in output


def test_sibling_agent_contract_absence_is_a_clean_skip(monkeypatch, tmp_path, capsys) -> None:
    missing = tmp_path / "mumei-agent" / "agent" / "mcp_server.py"
    monkeypatch.setattr("scripts.check_mcp_tool_contract.SIBLING_AGENT_SERVER", missing)
    assert main() == 0
    output = capsys.readouterr().out
    assert "skipped" in output
    assert "sibling repository unavailable" in output
