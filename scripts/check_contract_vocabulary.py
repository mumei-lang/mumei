#!/usr/bin/env python3
"""Check cross-project contract vocabulary in mumei docs.

The canonical source is docs/CROSS_PROJECT_ROADMAP.md.  This script keeps the
small fixed vocabulary mirrored by local docs from drifting into alternate audit
keys or a broader Lean fallback contract.
"""
from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CANONICAL_DOC = REPO_ROOT / "docs" / "CROSS_PROJECT_ROADMAP.md"
SYNC_DOCS = [
    REPO_ROOT / "docs" / "ROADMAP.md",
    REPO_ROOT / "docs" / "ONBOARDING.md",
    REPO_ROOT / "docs" / "PROOF_CERTIFICATE.md",
    REPO_ROOT / "README.md",
    REPO_ROOT / "instruction.md",
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


def main() -> int:
    violations = _check_canonical_doc()
    for path in SYNC_DOCS:
        if not path.exists():
            violations.append(Violation(path, "sync target is missing"))
            continue
        violations.extend(_check_forbidden_alias_keys(path))
        violations.extend(_check_meaning_contradictions(path))

    if violations:
        print("Contract vocabulary check failed:", file=sys.stderr)
        for violation in violations:
            print(f"- {violation.format()}", file=sys.stderr)
        return 1

    checked = ", ".join(path.relative_to(REPO_ROOT).as_posix() for path in SYNC_DOCS)
    print(f"Contract vocabulary check passed: {CANONICAL_DOC.relative_to(REPO_ROOT)} + {checked}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
