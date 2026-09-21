#!/usr/bin/env python3
"""Check cross-project vocabulary and Lean bridge contract constants.

The canonical source is docs/CROSS_PROJECT_ROADMAP.md.  This script keeps the
small fixed vocabulary mirrored by local docs from drifting into alternate audit
keys or a broader Lean fallback contract.

The generated schema/bridge_lemma_catalog.json mirror is also checked against
the Rust constants and proof-certificate fixture. ``--write`` regenerates those
targets from a catalog override, while the default ``--check`` mode is the CI
gate.

Since ws-cli-mcp this gate also checks:
- ``mcp_server.py`` tool docstrings for forbidden aliases
- ``src/cli.rs`` help/about strings for forbidden aliases
- both surfaces for ``contradiction_type`` alias drift

The MCP docstring extraction uses the ``ast`` module (standard library) to
reliably extract docstrings from all ``@mcp.tool``-decorated functions regardless
of decorator arguments, multi-line signatures, or blank lines before the
docstring.  A count assertion ensures silent extraction failures are caught.
"""
from __future__ import annotations

import ast
import hashlib
import json
import re
import shutil
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Pattern, Sequence

REPO_ROOT = Path(__file__).resolve().parents[1]
BRIDGE_LEMMA_CATALOG = REPO_ROOT / "schema" / "bridge_lemma_catalog.json"
TYPES_RS = REPO_ROOT / "mumei-core" / "src" / "verification" / "types.rs"
VERIFIED_SAMPLE_FIXTURE = (
    REPO_ROOT / "tests" / "fixtures" / "proof-cert" / "verified_sample.proof-cert.json"
)
CANONICAL_DOC = REPO_ROOT / "docs" / "CROSS_PROJECT_ROADMAP.md"
SYNC_DOCS = [
    REPO_ROOT / "docs" / "ROADMAP.md",
    REPO_ROOT / "docs" / "ONBOARDING.md",
    REPO_ROOT / "docs" / "PROOF_CERTIFICATE.md",
    REPO_ROOT / "docs" / "TRUSTED_ATOMS.md",
    REPO_ROOT / "docs" / "MCP_TOOL_CONTRACT.md",
    REPO_ROOT / "docs" / "LSP_DIAGNOSTIC_DATA.md",
    REPO_ROOT / "README.md",
    REPO_ROOT / "instruction.md",
]
NO_MM_LANGUAGE_DOCS = [
    CANONICAL_DOC,
    REPO_ROOT / "docs" / "ROADMAP.md",
    REPO_ROOT / "docs" / "ONBOARDING.md",
]
MCP_SERVER = REPO_ROOT / "mcp_server.py"
CLI_SOURCE = REPO_ROOT / "src" / "cli.rs"
CONTRADICTION_TYPE_ALIASES = [
    "contradiction_kind",
    "contradiction_class",
    "contradiction_category",
]

HARNESS_KEYS = [
    "harness_contract",
    "intent_fidelity",
    "artifact_paths",
    "budget_policy_fingerprint",
    "lean_verified",
]
NO_MM_KEYS = [
    "spec_health_issues",
    "verification_violations",
    "verification_status",
    "cross_validation_gaps",
    "next_steps",
    "migration_hints",
    "healed_files",
    "heal_errors",
]
FORBIDDEN_ALIASES = [
    "recommendations",
    "actions",
    "audit_issues",
    "verification_gaps",
    "repair_hints",
    "review_actions",
    "human_review",
]
REQUIRED_NO_MM_LANGUAGE_PHRASES = [
    "Python, Rust, TypeScript, Go, and Solidity",
    "parser path",
    "deterministic/no-LLM",
    "Rust `a + b` i64 overflow",
    "TypeScript `name!.length` null/undefined",
    "Go `values[idx]` bounds",
    "Z3 counterexample",
]


@dataclass(frozen=True)
class Violation:
    path: Path
    message: str
    line_number: int | None = None

    def format(self) -> str:
        rel = self.path.relative_to(REPO_ROOT)
        if self.line_number is None:
            return f"{rel}: {self.message}"
        return f"{rel}:{self.line_number}: {self.message}"


def load_bridge_lemma_catalog(path: Path = BRIDGE_LEMMA_CATALOG) -> dict:
    with path.open(encoding="utf-8") as handle:
        catalog = json.load(handle)
    if (
        not isinstance(catalog, dict)
        or not isinstance(catalog.get("translator_version"), str)
        or not isinstance(catalog.get("obligation_classes"), dict)
        or any(
            not isinstance(cls, str)
            or not isinstance(lemmas, list)
            or any(not isinstance(lemma, str) for lemma in lemmas)
            for cls, lemmas in catalog["obligation_classes"].items()
        )
    ):
        raise ValueError("invalid bridge lemma catalog")
    return catalog


def compute_bridge_lemma_hash(obligation_classes: dict[str, list[str]]) -> str:
    """Match mumei-lean ``expr_translator.bridge_lemma_hash_for`` exactly."""
    canonical = "\n".join(
        f"{obligation_class}:{lemma}"
        for obligation_class in sorted(obligation_classes)
        for lemma in sorted(obligation_classes[obligation_class])
    )
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


_TRANSLATOR_RE = re.compile(
    r'(?m)^pub const LEAN_TRANSLATOR_VERSION: &str = "([^"]*)";'
)
_HASH_RE = re.compile(
    r'(?m)^pub const LEAN_BRIDGE_LEMMA_HASH: &str =\s*\n\s*"([^"]*)";'
)
_JSON_TRANSLATOR_RE = re.compile(r'(?m)"translator_version"\s*:\s*"([^"]*)"')
_JSON_HASH_RE = re.compile(r'(?m)"bridge_lemma_hash"\s*:\s*"([^"]*)"')


def contract_constant_targets(
    translator_version: str, bridge_lemma_hash: str
) -> list[tuple[Path, str, Pattern[str], str]]:
    return [
        (TYPES_RS, "translator_version", _TRANSLATOR_RE, translator_version),
        (TYPES_RS, "bridge_lemma_hash", _HASH_RE, bridge_lemma_hash),
        (
            VERIFIED_SAMPLE_FIXTURE,
            "translator_version",
            _JSON_TRANSLATOR_RE,
            translator_version,
        ),
        (
            VERIFIED_SAMPLE_FIXTURE,
            "bridge_lemma_hash",
            _JSON_HASH_RE,
            bridge_lemma_hash,
        ),
    ]


def _replace_contract_target(
    path: Path,
    field: str,
    pattern: Pattern[str],
    expected: str,
    write: bool,
) -> list[Violation]:
    if not path.exists():
        return [Violation(path, "target is missing")]
    text = path.read_text(encoding="utf-8")
    matches = list(pattern.finditer(text))
    if not matches:
        return [Violation(path, f"{field} anchor not found")]
    violations = [
        Violation(path, f"{field} is {match.group(1)}, expected {expected}")
        for match in matches
        if match.group(1) != expected
    ]
    if write and violations:
        text = pattern.sub(
            lambda match: match.group(0)[: match.start(1) - match.start(0)]
            + expected
            + match.group(0)[match.end(1) - match.start(0) :],
            text,
        )
        path.write_text(text, encoding="utf-8")
    return [] if write else violations


def _sync_contract_constants(write: bool) -> list[Violation]:
    catalog = load_bridge_lemma_catalog()
    translator_version = catalog["translator_version"]
    bridge_lemma_hash = compute_bridge_lemma_hash(catalog["obligation_classes"])
    violations: list[Violation] = []
    for target in contract_constant_targets(translator_version, bridge_lemma_hash):
        violations.extend(_replace_contract_target(*target, write=write))
    return violations


def _line_number(text: str, needle: str) -> int | None:
    for number, line in enumerate(text.splitlines(), 1):
        if needle in line:
            return number
    return None


def _contract_section(text: str) -> str:
    start = text.index("## Canonical cross-project contract")
    end = text.index("## 現状サマリ", start)
    return text[start:end]


def _check_canonical_doc() -> list[Violation]:
    text = CANONICAL_DOC.read_text(encoding="utf-8")
    section = _contract_section(text)
    violations: list[Violation] = []

    table_terms = set(re.findall(r"`([a-z_]+)`", section.split("## No-`.mm` front door", 1)[0]))
    for key in HARNESS_KEYS + NO_MM_KEYS:
        if key not in table_terms:
            violations.append(Violation(CANONICAL_DOC, f"canonical table is missing `{key}`"))

    for key in NO_MM_KEYS:
        if key not in section.split("## No-`.mm` front door", 1)[1]:
            violations.append(Violation(CANONICAL_DOC, f"No-`.mm` section is missing `{key}`"))

    for alias in FORBIDDEN_ALIASES:
        if f"`{alias}`" not in section:
            violations.append(Violation(CANONICAL_DOC, f"forbidden alias list is missing `{alias}`"))

    required_phrases = [
        "audit -> migrate-suggest -> heal",
        "current `translator_version` and `bridge_lemma_hash`",
        "`stale_translator`",
    ]
    for phrase in required_phrases:
        if phrase not in section:
            violations.append(Violation(CANONICAL_DOC, f"canonical section is missing phrase: {phrase}"))

    return violations


def _alias_key_patterns(alias: str) -> list[re.Pattern[str]]:
    quoted = re.escape(alias)
    return [
        re.compile(rf"`{quoted}(?:\[\])?`"),
        re.compile(rf'"{quoted}(?:\[\])?"\s*:'),
        re.compile(rf"'{quoted}(?:\[\])?'\s*:"),
        re.compile(rf"(?m)^\s*[-*]?\s*{quoted}(?:\[\])?\s*:"),
        re.compile(rf"\b{quoted}\[\]"),
    ]


def _check_forbidden_alias_keys(path: Path) -> list[Violation]:
    text = path.read_text(encoding="utf-8")
    violations: list[Violation] = []
    for alias in FORBIDDEN_ALIASES:
        for pattern in _alias_key_patterns(alias):
            match = pattern.search(text)
            if match:
                violations.append(
                    Violation(
                        path,
                        f"forbidden alias appears as a contract key: `{alias}`",
                        _line_number(text, match.group(0)),
                    )
                )
                break
    return violations


def _check_doc_contradiction_type_aliases(path: Path) -> list[Violation]:
    """Detect `contradiction_type` alias drift inside a synced doc.

    Mirrors the MCP/CLI ``contradiction_type`` guard for prose docs so a doc
    cannot reintroduce ``contradiction_kind``/``_class``/``_category`` once it
    is part of the docs-sync surface.
    """
    text = path.read_text(encoding="utf-8")
    violations: list[Violation] = []
    for alias in CONTRADICTION_TYPE_ALIASES:
        if re.search(rf"\b{re.escape(alias)}\b", text):
            violations.append(
                Violation(
                    path,
                    f"doc contains contradiction_type alias: `{alias}`",
                    _line_number(text, alias),
                )
            )
    return violations


def _paragraphs(text: str) -> list[tuple[int, str]]:
    blocks: list[tuple[int, list[str]]] = []
    current: list[str] = []
    start = 1
    for number, line in enumerate(text.splitlines(), 1):
        if line.strip():
            if not current:
                start = number
            current.append(line)
        elif current:
            blocks.append((start, current))
            current = []
    if current:
        blocks.append((start, current))
    return [(line, " ".join(lines)) for line, lines in blocks]


def _check_meaning_contradictions(path: Path) -> list[Violation]:
    text = path.read_text(encoding="utf-8")
    violations: list[Violation] = []
    for line_number, paragraph in _paragraphs(text):
        normalized = paragraph.lower()
        mentions_non_unknown = any(term in normalized for term in ("`sat`", "`unsat`", "parser failure", "parser failures", "audit finding", "audit findings"))
        mentions_lean_fallback = "lean" in normalized and ("fallback" in normalized or "escalat" in normalized or "流す" in normalized)
        denies_fallback = any(term in normalized for term in ("must not", "not ", "only for z3 `unknown`", "only z3 `unknown`", "only the z3", "z3 `unknown` only"))
        if mentions_non_unknown and mentions_lean_fallback and not denies_fallback:
            violations.append(
                Violation(
                    path,
                    "Lean escalation must remain limited to Z3 `unknown`; this paragraph appears to route sat/unsat/parser/audit cases to Lean",
                    line_number,
                )
            )

        if "lean_verified" in normalized and "translator_version" in normalized and "bridge_lemma_hash" not in normalized:
            violations.append(
                Violation(
                    path,
                    "`lean_verified` meaning mentions `translator_version` without `bridge_lemma_hash`",
                    line_number,
                )
            )
        if "lean_verified" in normalized and "bridge_lemma_hash" in normalized and "translator_version" not in normalized:
            violations.append(
                Violation(
                    path,
                    "`lean_verified` meaning mentions `bridge_lemma_hash` without `translator_version`",
                    line_number,
                )
            )
    return violations


def _extract_mcp_docstrings_ast(text: str) -> list[str]:
    """Extract docstrings from all @mcp.tool-decorated functions via AST.

    This handles decorator arguments (@mcp.tool(name=...)), multi-line
    signatures, and blank lines before the docstring without false negatives.
    """
    tree = ast.parse(text)
    blocks: list[str] = []
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        for decorator in node.decorator_list:
            is_mcp_tool = False
            if isinstance(decorator, ast.Call):
                func = decorator.func
                if (
                    isinstance(func, ast.Attribute)
                    and func.attr == "tool"
                    and isinstance(func.value, ast.Attribute)
                    and func.value.attr == "mcp"
                ):
                    is_mcp_tool = True
                elif (
                    isinstance(func, ast.Attribute)
                    and func.attr == "tool"
                    and isinstance(func.value, ast.Name)
                    and func.value.id == "mcp"
                ):
                    is_mcp_tool = True
            elif isinstance(decorator, ast.Attribute):
                if (
                    decorator.attr == "tool"
                    and isinstance(decorator.value, ast.Name)
                    and decorator.value.id == "mcp"
                ):
                    is_mcp_tool = True
            if is_mcp_tool:
                docstring = ast.get_docstring(node)
                if docstring:
                    blocks.append(docstring)
                break
    return blocks


def _count_mcp_tool_decorators(text: str) -> int:
    """Count @mcp.tool( occurrences to cross-check against AST extraction."""
    return len(re.findall(r"@mcp\.tool\(", text))


def _extract_mcp_tool_names_ast(text: str) -> set[str]:
    """Return the set of tool names exposed by @mcp.tool() decorators in *text*.

    Uses the function name by default, but honors an explicit ``name=...``
    keyword argument when present. A tool may be decorated with multiple
    ``@mcp.tool()`` calls in the source; each name is collected.
    """
    tree = ast.parse(text)
    names: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        for decorator in node.decorator_list:
            tool_name: str | None = None
            if isinstance(decorator, ast.Call):
                func = decorator.func
                if isinstance(func, ast.Attribute) and func.attr == "tool":
                    if (
                        isinstance(func.value, ast.Attribute)
                        and func.value.attr == "mcp"
                    ) or (
                        isinstance(func.value, ast.Name)
                        and func.value.id == "mcp"
                    ):
                        tool_name = node.name
                        for kw in decorator.keywords:
                            if kw.arg == "name" and isinstance(
                                kw.value, ast.Constant
                            ) and isinstance(kw.value.value, str):
                                tool_name = kw.value.value
            elif isinstance(decorator, ast.Attribute):
                if (
                    decorator.attr == "tool"
                    and isinstance(decorator.value, ast.Name)
                    and decorator.value.id == "mcp"
                ):
                    tool_name = node.name
            if tool_name:
                names.add(tool_name)
    return names


def _check_mcp_forbidden_aliases(path: Path) -> list[Violation]:
    """Check MCP tool docstrings for forbidden aliases via AST extraction.

    Includes a count assertion: if the number of extracted docstrings does not
    match the number of @mcp.tool( decorators in the source, a violation is
    raised to prevent silent false-negatives.
    """
    if not path.exists():
        return [Violation(path, "MCP server file is missing")]
    text = path.read_text(encoding="utf-8")
    violations: list[Violation] = []

    decorator_count = _count_mcp_tool_decorators(text)
    docstring_blocks = _extract_mcp_docstrings_ast(text)

    if decorator_count != len(docstring_blocks):
        violations.append(
            Violation(
                path,
                f"MCP docstring extraction count mismatch: "
                f"{len(docstring_blocks)} docstrings extracted but "
                f"{decorator_count} @mcp.tool decorators found — "
                f"some tools may be missing docstrings or extraction failed",
            )
        )

    combined = "\n".join(docstring_blocks)
    if not combined:
        return violations

    for alias in FORBIDDEN_ALIASES:
        if re.search(rf"\b{re.escape(alias)}\b", combined):
            violations.append(
                Violation(
                    path,
                    f"MCP docstring contains forbidden alias: `{alias}`",
                    _line_number(text, alias),
                )
            )
    for alias in CONTRADICTION_TYPE_ALIASES:
        if re.search(rf"\b{re.escape(alias)}\b", combined):
            violations.append(
                Violation(
                    path,
                    f"MCP docstring contains contradiction_type alias: `{alias}`",
                    _line_number(text, alias),
                )
            )
    return violations


def _check_cli_forbidden_aliases(path: Path) -> list[Violation]:
    """Check CLI help/about strings in Rust source for forbidden aliases."""
    if not path.exists():
        return [Violation(path, "CLI source file is missing")]
    text = path.read_text(encoding="utf-8")
    violations: list[Violation] = []
    for alias in FORBIDDEN_ALIASES:
        if re.search(rf"\b{re.escape(alias)}\b", text):
            violations.append(
                Violation(
                    path,
                    f"CLI help contains forbidden alias: `{alias}`",
                    _line_number(text, alias),
                )
            )
    for alias in CONTRADICTION_TYPE_ALIASES:
        if re.search(rf"\b{re.escape(alias)}\b", text):
            violations.append(
                Violation(
                    path,
                    f"CLI help contains contradiction_type alias: `{alias}`",
                    _line_number(text, alias),
                )
            )
    return violations


def _check_no_mm_language_sync(path: Path) -> list[Violation]:
    text = path.read_text(encoding="utf-8")
    normalized = " ".join(text.split()).lower()
    violations: list[Violation] = []
    for phrase in REQUIRED_NO_MM_LANGUAGE_PHRASES:
        if phrase.lower() not in normalized:
            violations.append(Violation(path, f"no-.mm language sync is missing phrase: {phrase}"))
    for key in NO_MM_KEYS:
        if key not in text:
            violations.append(Violation(path, f"no-.mm language sync is missing `{key}`"))
    return violations


def main(argv: Sequence[str] | None = None) -> int:
    import argparse

    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--write", action="store_true")
    parser.add_argument("--catalog", type=Path, default=BRIDGE_LEMMA_CATALOG)
    args = parser.parse_args([] if argv is None else argv)

    violations = _check_canonical_doc()
    for path in SYNC_DOCS:
        if not path.exists():
            violations.append(Violation(path, "sync target is missing"))
            continue
        violations.extend(_check_forbidden_alias_keys(path))
        violations.extend(_check_meaning_contradictions(path))
        violations.extend(_check_doc_contradiction_type_aliases(path))
    for path in NO_MM_LANGUAGE_DOCS:
        violations.extend(_check_no_mm_language_sync(path))

    # CLI help and MCP docstring forbidden-alias checks
    violations.extend(_check_mcp_forbidden_aliases(MCP_SERVER))
    violations.extend(_check_cli_forbidden_aliases(CLI_SOURCE))
    if args.catalog != BRIDGE_LEMMA_CATALOG:
        catalog = load_bridge_lemma_catalog(args.catalog)
        translator_version = catalog["translator_version"]
        bridge_lemma_hash = compute_bridge_lemma_hash(catalog["obligation_classes"])
        targets = contract_constant_targets(translator_version, bridge_lemma_hash)
        for path, field, pattern, expected in targets:
            target_path = path.relative_to(REPO_ROOT)
            violations.extend(
                _replace_contract_target(
                    REPO_ROOT / target_path,
                    field,
                    pattern,
                    expected,
                    write=args.write,
                )
            )
        if args.write:
            shutil.copyfile(args.catalog, BRIDGE_LEMMA_CATALOG)
    else:
        violations.extend(_sync_contract_constants(args.write))

    if violations:
        print("Contract vocabulary check failed:", file=sys.stderr)
        for violation in violations:
            print(f"- {violation.format()}", file=sys.stderr)
        return 1

    checked = ", ".join(path.relative_to(REPO_ROOT).as_posix() for path in SYNC_DOCS)
    extra = ", ".join(
        p.relative_to(REPO_ROOT).as_posix()
        for p in (MCP_SERVER, CLI_SOURCE)
        if p.exists()
    )
    print(
        f"Contract vocabulary check passed: "
        f"{CANONICAL_DOC.relative_to(REPO_ROOT)} + {checked} + {extra}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
