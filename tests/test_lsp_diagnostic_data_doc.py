from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
LSP_SOURCE = ROOT / "src" / "lsp.rs"
DOC = ROOT / "docs" / "LSP_DIAGNOSTIC_DATA.md"
# The pending escalation object is constructed separately and inserted into
# ``data`` dynamically, so its nested key is explicitly allowlisted here.
MECHANICAL_DATA_KEY_ALLOWLIST = {"escalation_reason"}


def _emitted_sources_and_data_keys() -> tuple[set[str], set[str]]:
    """Scan literal source fields, data.insert calls, and literal data objects."""
    source = LSP_SOURCE.read_text(encoding="utf-8")
    sources = set(re.findall(r'"source"\s*:\s*"([^"]+)"', source))
    keys = set(re.findall(r'data\.insert\("([^"]+)"\.to_string\(\)', source))
    for data_object in re.findall(r'"data"\s*:\s*\{(.*?)\}', source, re.DOTALL):
        keys.update(re.findall(r'"([^"]+)"\s*:', data_object))
    for key in MECHANICAL_DATA_KEY_ALLOWLIST:
        assert f'"{key}"' in source, f"allowlisted key is no longer emitted: {key}"
    keys.update(MECHANICAL_DATA_KEY_ALLOWLIST)
    return sources, keys


def _documented_sources_and_data_keys() -> tuple[set[str], set[str]]:
    doc = DOC.read_text(encoding="utf-8")
    sources = set(re.findall(r"\|\s*`(mumei(?:-[a-z0-9-]+)?)`\s*\|", doc))
    keys = {
        key
        for key in (
            "counterexample",
            "lean_escalation",
            "status",
            "atom",
            "z3_result_class",
            "escalation_reason",
            "certificate",
            "intentDrift",
            "kind",
            "score",
            "clause",
        )
        if (
            re.search(rf"`{re.escape(key)}`", doc)
            or re.search(rf'"{re.escape(key)}"\s*:', doc)
        )
    }
    return sources, keys


def test_lsp_sources_and_data_keys_match_documentation() -> None:
    emitted_sources, emitted_keys = _emitted_sources_and_data_keys()
    documented_sources, documented_keys = _documented_sources_and_data_keys()
    assert emitted_sources == documented_sources, (
        f"source documentation drift: emitted={sorted(emitted_sources)}, "
        f"documented={sorted(documented_sources)}"
    )
    assert emitted_keys == documented_keys, (
        f"data documentation drift: emitted={sorted(emitted_keys)}, "
        f"documented={sorted(documented_keys)}"
    )
