#!/usr/bin/env python3
"""Check the canonical mumei-forge MCP tool table against mcp_server.py."""
from __future__ import annotations

import ast
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MCP_SERVER = REPO_ROOT / "mcp_server.py"
CONTRACT_DOC = REPO_ROOT / "docs" / "MCP_TOOL_CONTRACT.md"


def _is_tool_decorator(node: ast.expr) -> bool:
    return (
        isinstance(node, ast.Call)
        and isinstance(node.func, ast.Attribute)
        and isinstance(node.func.value, ast.Name)
        and node.func.value.id == "mcp"
        and node.func.attr == "tool"
    )


def _annotation(node: ast.arg) -> str:
    return ast.unparse(node.annotation) if node.annotation else "Any"


def _signature(node: ast.FunctionDef | ast.AsyncFunctionDef) -> str:
    positional = node.args.posonlyargs + node.args.args
    defaults = [None] * (len(positional) - len(node.args.defaults))
    defaults += list(node.args.defaults)
    parts = []
    for arg, default in zip(positional, defaults):
        value = f"{arg.arg}: {_annotation(arg)}"
        if default is not None:
            value += f" = {ast.unparse(default)}"
        parts.append(value)
    if node.args.vararg:
        parts.append(f"*{node.args.vararg.arg}")
    for arg, default in zip(node.args.kwonlyargs, node.args.kw_defaults):
        value = f"{arg.arg}: {_annotation(arg)}"
        if default is not None:
            value += f" = {ast.unparse(default)}"
        parts.append(value)
    if node.args.kwarg:
        parts.append(f"**{node.args.kwarg.arg}")
    return ", ".join(parts)


def extract_tools(source: str) -> dict[str, str]:
    tree = ast.parse(source)
    tools: dict[str, str] = {}
    decorator_count = 0
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if any(_is_tool_decorator(decorator) for decorator in node.decorator_list):
            decorator_count += sum(
                _is_tool_decorator(decorator) for decorator in node.decorator_list
            )
            tools[node.name] = _signature(node)
    literal_count = len(re.findall(r"@mcp\.tool\(", source))
    if literal_count == 0 or decorator_count != literal_count:
        raise ValueError(
            "mcp.tool decorator extraction count mismatch: "
            f"literal={literal_count}, ast={decorator_count}"
        )
    return tools


def extract_documented_tools(text: str) -> dict[str, str]:
    section = text.split("## `mumei-agent`", 1)[0]
    section = section.split("## `mumei-forge`", 1)[1]
    tools = {}
    for line in section.splitlines():
        match = re.match(r"^\|\s*`([^`]+)`\s*\|\s*(.*)\s*\|$", line)
        if not match or match.group(1) in {"Tool", "---"}:
            continue
        name, remainder = match.groups()
        remainder = remainder.strip()
        if remainder.startswith("|"):
            arguments = ""
        elif remainder.startswith("`"):
            end = remainder.find("`", 1)
            if end < 0:
                continue
            arguments = remainder[1:end]
        else:
            arguments = remainder.split(" | ", 1)[0]
        tools[name] = arguments.strip().replace('"', "'")
    return tools


def main() -> int:
    try:
        actual = extract_tools(MCP_SERVER.read_text(encoding="utf-8"))
        documented = extract_documented_tools(CONTRACT_DOC.read_text(encoding="utf-8"))
    except (OSError, SyntaxError, ValueError) as exc:
        print(f"MCP tool contract check failed: {exc}", file=sys.stderr)
        return 1
    expected = {name: signature.replace('"', "'") for name, signature in actual.items()}
    failures = []
    for name in sorted(set(expected) - set(documented)):
        failures.append(f"missing tool in contract: {name}")
    for name in sorted(set(documented) - set(expected)):
        failures.append(f"extra tool in contract: {name}")
    for name in sorted(set(expected) & set(documented)):
        if expected[name] != documented[name]:
            failures.append(
                f"signature mismatch for {name}: expected {expected[name]!r}, "
                f"documented {documented[name]!r}"
            )
    if failures:
        print("MCP tool contract check failed:", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print(f"MCP tool contract check passed ({len(expected)} mumei-forge tools)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
