"""Tests for the enhanced list_std_catalog() function in mcp_server.py."""
from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

# Add project root to path so we can import mcp_server
sys.path.insert(0, str(Path(__file__).parent.parent))


def _get_catalog() -> dict:
    """Call list_std_catalog() and parse the JSON result."""
    # Import inline to avoid MCP server startup side effects
    from mcp_server import list_std_catalog
    raw = list_std_catalog()
    return json.loads(raw)


class TestListStdCatalog:
    """Tests for the enhanced list_std_catalog() function."""

    def test_discovers_all_std_modules(self) -> None:
        """All .mm files under std/ are discovered."""
        catalog = _get_catalog()
        assert "modules" in catalog
        modules = catalog["modules"]
        assert len(modules) > 0

        # Verify known modules exist
        paths = {m["path"] for m in modules}
        assert "std/contracts.mm" in paths
        assert "std/prelude.mm" in paths

    def test_contracts_has_expected_types(self) -> None:
        """std/contracts.mm has the expected refinement types."""
        catalog = _get_catalog()
        contracts = next(
            m for m in catalog["modules"] if m["path"] == "std/contracts.mm"
        )
        type_names = [t.split("=")[0].strip() for t in contracts["types"]]
        assert "type Port" in type_names
        assert "type NonNegative" in type_names

    def test_contracts_has_expected_atoms(self) -> None:
        """std/contracts.mm has atoms with requires/ensures fields."""
        catalog = _get_catalog()
        contracts = next(
            m for m in catalog["modules"] if m["path"] == "std/contracts.mm"
        )
        atoms = contracts["atoms"]
        assert len(atoms) > 0

        # Each atom should be a dict with signature, requires, ensures
        for atom in atoms:
            assert "signature" in atom
            assert "requires" in atom
            assert "ensures" in atom
            assert "effects" in atom

        # Check a specific atom
        clamp = next(
            (a for a in atoms if "clamp" in a["signature"]), None
        )
        assert clamp is not None
        assert clamp["requires"] != ""
        assert clamp["ensures"] != ""

    def test_effects_mm_has_all_effect_forms(self) -> None:
        """std/effects.mm captures non-parameterized, parameterized, and composite effects."""
        catalog = _get_catalog()
        effects_mod = next(
            (m for m in catalog["modules"] if m["path"] == "std/effects.mm"),
            None,
        )
        assert effects_mod is not None, "std/effects.mm not found in catalog"

        effect_names = [e.split("(")[0].split(" includes")[0].strip() for e in effects_mod["effects"]]

        # Non-parameterized effects
        assert "effect FileRead" in effect_names
        assert "effect FileWrite" in effect_names
        assert "effect Network" in effect_names
        assert "effect Log" in effect_names
        assert "effect Console" in effect_names

        # Parameterized effects
        assert "effect HttpGet" in effect_names
        assert "effect HttpPost" in effect_names

        # Composite effects
        composite = [e for e in effects_mod["effects"] if "includes:" in e]
        assert len(composite) >= 3, f"Expected >=3 composite effects, got {composite}"

    def test_http_secure_has_effects(self) -> None:
        """std/http_secure.mm has effect definitions."""
        catalog = _get_catalog()
        modules_by_path = {m["path"]: m for m in catalog["modules"]}

        # Find http_secure module if it exists
        http_mod = modules_by_path.get("std/http_secure.mm")
        if http_mod is None:
            pytest.skip("std/http_secure.mm not found")

        # Should have effects list
        assert "effects" in http_mod

    def test_module_has_description(self) -> None:
        """Modules with leading comments have a description field."""
        catalog = _get_catalog()
        contracts = next(
            m for m in catalog["modules"] if m["path"] == "std/contracts.mm"
        )
        assert "description" in contracts
        # contracts.mm has a leading comment block
        assert contracts["description"] != ""

    def test_module_entry_has_all_fields(self) -> None:
        """Each module entry has the expected fields."""
        catalog = _get_catalog()
        for module in catalog["modules"]:
            assert "path" in module
            assert "import" in module
            assert "description" in module
            assert "types" in module
            assert "atoms" in module
            assert "structs" in module
            assert "effects" in module
