#!/usr/bin/env python3
"""Generate Rust proptest property tests from FFI atom contracts.

Scans std/*.mm for FFI-backed `atom` / `trusted atom` declarations, extracts
requires/ensures contracts, and emits proptest-based Rust integration tests
that exercise the real FFI backend implementations.

Usage:
    python scripts/ffi_contract_test_gen.py
    python scripts/ffi_contract_test_gen.py -o outdir   # custom output directory
    python scripts/ffi_contract_test_gen.py --dry-run   # preview without writing

Part of the Reduction Roadmap Step 1 (docs/TRUSTED_ATOMS.md).
"""
from __future__ import annotations

import argparse
import re
import sys
import textwrap
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
STD_DIR = REPO_ROOT / "std"

# ---------------------------------------------------------------------------
# Parsing helpers
# ---------------------------------------------------------------------------

FFI_ATOM_RE = re.compile(
    r"^\s*(?:trusted\s+)?atom\s+(\w+)\s*\(([^)]*)\)",
    re.MULTILINE,
)

FFI_ATOM_NAMES = {
    "file": {"read_file", "write_file", "exists", "remove"},
    "http": {
        "get",
        "post",
        "put",
        "delete",
        "status",
        "body",
        "body_json",
        "header_get",
        "header_set",
        "is_ok",
        "is_error",
        "free",
    },
    "http_secure": {
        "secure_get",
        "secure_post",
        "secure_put",
        "secure_delete",
        "status",
        "body",
        "is_ok",
        "free",
    },
    "http_server": {"bind_server", "listen_server", "accept_request", "send_response"},
    "json": {
        "parse",
        "stringify",
        "get",
        "get_int",
        "get_str",
        "get_bool",
        "array_len",
        "array_get",
        "is_null",
        "is_object",
        "is_array",
        "object_new",
        "object_set",
        "array_new",
        "array_push",
        "from_int",
        "from_str",
        "from_bool",
        "free",
        "str_free",
    },
}

@dataclass
class Param:
    name: str
    ty: str  # "i64" or "Str"


@dataclass
class TrustedAtom:
    name: str
    params: list[Param]
    requires: str
    ensures: str
    module: str  # e.g. "json", "http"
    ffi_fn: str  # e.g. "json_parse"


def _parse_params(raw: str) -> list[Param]:
    params: list[Param] = []
    for chunk in raw.split(","):
        chunk = chunk.strip()
        if not chunk:
            continue
        parts = chunk.split(":")
        if len(parts) == 2:
            params.append(Param(name=parts[0].strip(), ty=parts[1].strip()))
    return params


def _extract_clause(text: str, start: int, clause_name: str) -> str:
    """Extract a clause value terminated by `;` from *text* starting at *start*."""
    # Find the clause keyword followed by ':'
    pattern = re.compile(
        rf"^\s*{clause_name}\s*:\s*",
        re.MULTILINE,
    )
    m = pattern.search(text, start)
    if m is None:
        return "true"
    value_start = m.end()
    # Clause value runs until the first `;` that is not inside braces
    depth = 0
    i = value_start
    while i < len(text):
        ch = text[i]
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
        elif ch == ";" and depth == 0:
            return text[value_start:i].strip()
        i += 1
    return text[value_start:].strip().rstrip(";")


def _atom_ffi_fn(module: str, atom_name: str) -> str:
    """Map a module+atom name to the extern "C" FFI function name."""
    prefix_map = {
        "json": "json_",
        "http": "http_",
        "http_secure": "http_",
        "http_server": "http_server_",
        "file": "file_",
    }
    prefix = prefix_map.get(module, "")

    # Special cases where the atom name doesn't directly map
    special_map = {
        ("json", "str_free"): "string_free",
        ("file", "read_file"): "file_read",
        ("file", "write_file"): "file_write",
        ("file", "remove"): "file_delete",
        ("http_server", "bind_server"): "http_server_bind",
        ("http_server", "listen_server"): "http_server_listen",
        ("http_server", "accept_request"): "http_server_accept",
        ("http_server", "send_response"): "http_server_respond",
    }
    if (module, atom_name) in special_map:
        return special_map[(module, atom_name)]

    # For http_secure, strip the "secure_" prefix for the underlying fn
    if module == "http_secure":
        if atom_name.startswith("secure_"):
            return f"http_{atom_name[7:]}"
        return f"http_{atom_name}"

    return f"{prefix}{atom_name}"


def parse_mm_file(path: Path) -> list[TrustedAtom]:
    """Extract FFI-backed atom declarations from a .mm file."""
    text = path.read_text(encoding="utf-8")
    module = path.stem  # e.g. "json", "http"
    atoms: list[TrustedAtom] = []

    for m in FFI_ATOM_RE.finditer(text):
        name = m.group(1)
        if name not in FFI_ATOM_NAMES.get(module, set()):
            continue
        params = _parse_params(m.group(2))
        atom_start = m.start()
        requires = _extract_clause(text, atom_start, "requires")
        ensures = _extract_clause(text, atom_start, "ensures")
        ffi_fn = _atom_ffi_fn(module, name)
        atoms.append(
            TrustedAtom(
                name=name,
                params=params,
                requires=requires,
                ensures=ensures,
                module=module,
                ffi_fn=ffi_fn,
            )
        )
    return atoms


# ---------------------------------------------------------------------------
# Rust code generation
# ---------------------------------------------------------------------------

def _is_str_param(p: Param) -> bool:
    return p.ty == "Str"


def _translate_requires_to_strategy(atom: TrustedAtom) -> list[str]:
    """Generate proptest strategy lines for each parameter based on requires."""
    lines: list[str] = []
    req = atom.requires.strip()
    handle_params = set(_handle_params(atom))

    for p in atom.params:
        if p.name in handle_params:
            continue
        if p.name == "url" and atom.module in {"http", "http_secure"}:
            continue
        if p.name == "addr" and atom.module == "http_server":
            continue
        if p.name == "content" and atom.module == "file":
            continue
        if _is_str_param(p):
            # Check for starts_with constraint
            sw_match = re.search(
                rf'starts_with\s*\(\s*{re.escape(p.name)}\s*,\s*"([^"]+)"\s*\)',
                req,
            )
            if sw_match:
                prefix = sw_match.group(1)
                lines.append(
                    f'            suffix in "[a-zA-Z0-9._/\\\\-]{{0,64}}",',
                )
            else:
                lines.append(
                    f'            {p.name}_str in "[a-zA-Z0-9 _.,]{{0,128}}",',
                )
        else:
            # i64 parameter — look for range constraints
            lower = _find_lower_bound(req, p.name)
            upper = _find_upper_bound(req, p.name)
            lo = lower if lower is not None else 0
            hi = upper if upper is not None else 1000
            lines.append(f"            {p.name} in {lo}i64..={hi}i64,")
    return lines


def _find_lower_bound(req: str, param: str) -> int | None:
    """Extract lower bound from requires like `param > 0` or `param >= 1`."""
    m = re.search(rf"\b{re.escape(param)}\s*>\s*(\d+)", req)
    if m:
        return int(m.group(1)) + 1
    m = re.search(rf"\b{re.escape(param)}\s*>=\s*(\d+)", req)
    if m:
        return int(m.group(1))
    return None


def _find_upper_bound(req: str, param: str) -> int | None:
    """Extract upper bound from requires like `param <= 599`."""
    m = re.search(rf"\b{re.escape(param)}\s*<=\s*(\d+)", req)
    if m:
        return int(m.group(1))
    m = re.search(rf"\b{re.escape(param)}\s*<\s*(\d+)", req)
    if m:
        return int(m.group(1)) - 1
    return None


def _translate_ensures_to_assertions(atom: TrustedAtom) -> list[str]:
    """Generate Rust assertion lines from ensures clause."""
    ens = atom.ensures.strip()
    if ens == "true":
        return [
            "    prop_assert!(mumei_ffi_tests::contract_result_observed(&result));"
        ]

    assertions: list[str] = []
    # Split on top-level && for conjuncts. Avoid splitting conjunctions nested
    # inside parentheses, e.g. `result == 0 || (result >= 100 && result <= 599)`.
    parts = _split_top_level_and(ens)
    for part in parts:
        part = part.strip()
        if not part or part == "true":
            continue
        # result >= N
        m = re.match(r"result\s*>=\s*(\d+)", part)
        if m:
            assertions.append(
                f'    prop_assert!(result >= {m.group(1)}, "ensures: result >= {m.group(1)}, got {{}}", result);'
            )
            continue
        # result <= N
        m = re.match(r"result\s*<=\s*(\d+)", part)
        if m:
            assertions.append(
                f'    prop_assert!(result <= {m.group(1)}, "ensures: result <= {m.group(1)}, got {{}}", result);'
            )
            continue
        # result > N
        m = re.match(r"result\s*>\s*(\d+)", part)
        if m:
            assertions.append(
                f'    prop_assert!(result > {m.group(1)}, "ensures: result > {m.group(1)}, got {{}}", result);'
            )
            continue
        # result == expr  (rare but handle)
        m = re.match(r"result\s*==\s*(.+)", part)
        if m:
            if "result >= 100" in part and "result <= 599" in part:
                assertions.append(
                    '    prop_assert!(result == 0 || (result >= 100 && result <= 599), "ensures: result == 0 || (100 <= result <= 599), got {}", result);'
                )
                continue
            assertions.append(
                f"    // ensures: result == {m.group(1)} — skipped (depends on input semantics)"
            )
            continue
        assertions.append(f"    // ensures: {part} — cannot auto-translate")

    return assertions if assertions else [
        "    // ensures: complex — manual review needed"
    ]


def _split_top_level_and(expr: str) -> list[str]:
    parts: list[str] = []
    depth = 0
    current: list[str] = []
    i = 0
    while i < len(expr):
        ch = expr[i]
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth = max(0, depth - 1)
        if depth == 0 and expr.startswith("&&", i):
            part = "".join(current).strip()
            if part:
                parts.append(part)
            current = []
            i += 2
            continue
        current.append(ch)
        i += 1
    part = "".join(current).strip()
    if part:
        parts.append(part)
    return parts


def _handle_params(atom: TrustedAtom) -> list[str]:
    """Return names of params that represent handles (require > 0)."""
    req = atom.requires
    handles = []
    for p in atom.params:
        if (
            not _is_str_param(p)
            and (p.name == "handle" or p.name.endswith("_handle") or p.name == "path")
            and (
                re.search(rf"\b{re.escape(p.name)}\s*>\s*0", req)
                or re.search(rf"\b{re.escape(p.name)}\s*>=\s*1\b", req)
            )
        ):
            handles.append(p.name)
    return handles


def _generate_handle_setup(atom: TrustedAtom) -> list[str]:
    """Generate code to obtain valid handles before the test call."""
    lines: list[str] = []
    handle_params = _handle_params(atom)
    if not handle_params:
        return lines

    module = atom.module

    for hp in handle_params:
        if module == "json":
            if hp == "handle":
                if atom.name == "str_free":
                    lines.append(f'    let {hp} = mumei_ffi_tests::string_handle("contract");')
                elif atom.name in {"array_len", "array_get", "array_push"}:
                    lines.append(f"    let {hp} = mumei_ffi_tests::json_array_handle();")
                else:
                    lines.append(f"    let {hp} = mumei_ffi_tests::json_object_handle();")
            elif hp == "value":
                lines.append(
                    f"    let {hp} = mumei_core::ffi::json::json_from_int({hp});"
                )
            lines.append(f"    prop_assume!({hp} > 0);")
        elif module in ("http", "http_secure"):
            lines.append(f"    let {hp} = mumei_ffi_tests::http_response_handle();")
            lines.append(f"    prop_assume!({hp} > 0);")
        elif module == "http_server":
            if hp == "server_handle":
                if atom.name == "accept_request":
                    lines.append(
                        f"    let ({hp}, client_for_accept_cleanup) = mumei_ffi_tests::server_handle_with_pending_client();"
                    )
                else:
                    lines.append(f"    let {hp} = mumei_ffi_tests::server_handle();")
                lines.append(f"    prop_assume!({hp} > 0);")
            elif hp == "req_handle":
                lines.append(
                    "    let (server_handle_for_cleanup, req_handle, client_for_request_cleanup) = mumei_ffi_tests::server_request_handle();"
                )
                lines.append("    prop_assume!(server_handle_for_cleanup > 0);")
                lines.append("    prop_assume!(req_handle > 0);")
        elif module == "file":
            if hp == "path":
                lines.append(
                    "    let (path, temp_path_for_cleanup) = mumei_ffi_tests::temp_path_handle(\"path\");"
                )
                if atom.name in {"read_file", "exists", "remove"}:
                    lines.append(
                        '    std::fs::write(&temp_path_for_cleanup, "contract").unwrap();'
                    )
                lines.append("    prop_assume!(path > 0);")
            elif hp == "content":
                lines.append('    let content = mumei_ffi_tests::string_handle("contract");')
                lines.append("    prop_assume!(content > 0);")
    if module == "file" and atom.name == "write_file":
        lines.append('    let content = mumei_ffi_tests::string_handle("contract");')
        lines.append("    prop_assume!(content > 0);")
    return lines


def _generate_call(atom: TrustedAtom) -> str:
    """Generate the FFI function call expression."""
    args: list[str] = []
    for p in atom.params:
        if _is_str_param(p):
            # Check for starts_with constraint
            sw_match = re.search(
                rf'starts_with\s*\(\s*{re.escape(p.name)}\s*,\s*"([^"]+)"\s*\)',
                atom.requires,
            )
            if sw_match:
                args.append(f"{p.name}_c.as_ptr()")
            else:
                args.append(f"{p.name}_c.as_ptr()")
        else:
            args.append(p.name)

    ffi_mod = {
        "json": "json",
        "http": "http",
        "http_secure": "http",
        "http_server": "http_server",
        "file": "file",
    }.get(atom.module, atom.module)

    return f"mumei_core::ffi::{ffi_mod}::{atom.ffi_fn}({', '.join(args)})"


def _generate_str_prep(atom: TrustedAtom) -> list[str]:
    """Generate CString preparation lines for Str parameters."""
    lines: list[str] = []
    for p in atom.params:
        if _is_str_param(p):
            if p.name == "url" and atom.module == "http":
                lines.append(f"    let {p.name}_c = mumei_ffi_tests::local_http_url();")
                continue
            if p.name == "url" and atom.module == "http_secure":
                lines.append(f"    let {p.name}_c = mumei_ffi_tests::https_error_url();")
                continue
            if p.name == "addr" and atom.module == "http_server":
                lines.append(f"    let {p.name}_c = mumei_ffi_tests::unique_local_addr();")
                continue
            sw_match = re.search(
                rf'starts_with\s*\(\s*{re.escape(p.name)}\s*,\s*"([^"]+)"\s*\)',
                atom.requires,
            )
            if sw_match:
                prefix = sw_match.group(1)
                lines.append(
                    f'    let {p.name}_raw = format!("{prefix}{{}}", suffix);'
                )
                lines.append(
                    f"    let {p.name}_c = std::ffi::CString::new({p.name}_raw).unwrap();"
                )
            else:
                lines.append(
                    f"    let {p.name}_filtered: String = {p.name}_str.chars().filter(|c| *c != '\\0').collect();"
                )
                lines.append(
                    f"    let {p.name}_c = std::ffi::CString::new({p.name}_filtered).unwrap();"
                )
    return lines


def generate_test_fn(atom: TrustedAtom) -> str:
    """Generate a single proptest test function for one trusted atom."""
    fn_name = f"contract_{atom.module}_{atom.name}"
    strategy_lines = _translate_requires_to_strategy(atom)
    ensure_lines = _translate_ensures_to_assertions(atom)
    str_prep = _generate_str_prep(atom)
    handle_setup = _generate_handle_setup(atom)
    if not strategy_lines:
        strategy_lines = ["            _case in 0u8..=0u8,"]

    # Build the proptest body
    body_lines: list[str] = []
    body_lines.extend(str_prep)
    body_lines.extend(handle_setup)

    call_expr = _generate_call(atom)
    body_lines.append(f"    let result = {call_expr};")
    body_lines.extend(ensure_lines)

    # Handle cleanup for json handles created during setup
    handle_params = _handle_params(atom)
    for hp in handle_params:
        if atom.module == "json" or (
            atom.module in ("http", "http_secure") and hp == "handle"
        ):
            if atom.module == "json" and atom.name in {"free", "str_free"}:
                continue
            if atom.module in {"http", "http_secure"}:
                if atom.name == "free":
                    continue
                body_lines.append(f"    mumei_core::ffi::http::http_free({hp});")
            else:
                body_lines.append(f"    mumei_core::ffi::json::json_free({hp});")
        elif atom.module == "http_server" and hp == "server_handle":
            if atom.name == "accept_request":
                body_lines.append(
                    "    if result > 0 { mumei_core::ffi::http_server::http_request_free(result); }"
                )
                body_lines.append("    let _ = client_for_accept_cleanup.join();")
            body_lines.append(
                f"    mumei_core::ffi::http_server::http_server_free({hp});"
            )
        elif atom.module == "http_server" and hp == "req_handle":
            body_lines.append(
                "    mumei_core::ffi::http_server::http_request_free(req_handle);"
            )
            body_lines.append("    let _ = client_for_request_cleanup.join();")
            body_lines.append(
                "    mumei_core::ffi::http_server::http_server_free(server_handle_for_cleanup);"
            )
        elif atom.module == "file" and hp == "path":
            body_lines.append("    let _ = std::fs::remove_file(temp_path_for_cleanup);")
            body_lines.append("    mumei_core::ffi::json::mumei_str_free(path);")
        elif atom.module == "file" and hp == "content":
            body_lines.append("    mumei_core::ffi::json::mumei_str_free(content);")

    if atom.module == "file" and atom.name == "write_file":
        body_lines.append("    mumei_core::ffi::json::mumei_str_free(content);")

    if strategy_lines:
        strategy_block = "\n".join(strategy_lines)
        proptest_head = f"        fn {fn_name}(\n{strategy_block}\n        ) {{"
    else:
        proptest_head = f"        fn {fn_name}() {{"

    body_block = "\n".join(body_lines)

    return (
        "proptest! {\n"
        "    #[test]\n"
        f"{proptest_head}\n"
        f"{body_block}\n"
        "    }\n"
        "}\n"
    )


def _should_skip_atom(atom: TrustedAtom) -> tuple[bool, str]:
    """Return (skip, reason) if the atom should be skipped for test generation."""
    return False, ""


def generate_module_file(module: str, atoms: list[TrustedAtom]) -> str:
    """Generate a complete Rust test file for a module."""
    header = textwrap.dedent(f"""\
    //! Auto-generated FFI contract property tests for std/{module}.mm
    //!
    //! Generated by scripts/ffi_contract_test_gen.py
    //! DO NOT EDIT — regenerate with:
    //!   python scripts/ffi_contract_test_gen.py
    //!
    //! Tests verify that the Rust FFI backend satisfies the
    //! requires/ensures contracts declared in the .mm source.

    use proptest::prelude::*;

    """)

    test_fns: list[str] = []
    skipped: list[str] = []

    for atom in atoms:
        skip, reason = _should_skip_atom(atom)
        if skip:
            skipped.append(f"// SKIPPED: {atom.name} — {reason}")
            continue
        test_fns.append(generate_test_fn(atom))

    skip_block = ""
    if skipped:
        skip_block = (
            "// The following atoms are skipped for automated property testing:\n"
            + "\n".join(skipped)
            + "\n\n"
        )

    return header + skip_block + "\n".join(test_fns)


def generate_report(all_atoms: list[TrustedAtom]) -> str:
    """Generate a summary report of test generation."""
    total = len(all_atoms)
    skipped = sum(1 for a in all_atoms if _should_skip_atom(a)[0])
    generated = total - skipped

    lines = [
        f"FFI Contract Test Generation Report",
        f"====================================",
        f"Total trusted atoms scanned: {total}",
        f"Tests generated:             {generated}",
        f"Atoms skipped:               {skipped}",
        f"",
        f"Breakdown by module:",
    ]
    by_module: dict[str, tuple[int, int]] = {}
    for a in all_atoms:
        skip, _ = _should_skip_atom(a)
        key = a.module
        gen, sk = by_module.get(key, (0, 0))
        if skip:
            by_module[key] = (gen, sk + 1)
        else:
            by_module[key] = (gen + 1, sk)
    for mod_name in sorted(by_module):
        gen, sk = by_module[mod_name]
        lines.append(f"  {mod_name:20s}: {gen} generated, {sk} skipped")

    return "\n".join(lines)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate Rust proptest tests from trusted atom contracts"
    )
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=REPO_ROOT / "tests" / "ffi_contracts",
        help="Output directory for generated test files",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print generated code without writing files",
    )
    parser.add_argument(
        "--report",
        action="store_true",
        help="Print a summary report",
    )
    args = parser.parse_args()

    mm_files = sorted(STD_DIR.glob("*.mm"))
    target_modules = {"json", "http", "http_secure", "http_server", "file"}

    all_atoms: list[TrustedAtom] = []
    by_module: dict[str, list[TrustedAtom]] = {}

    for mm_path in mm_files:
        module = mm_path.stem
        if module not in target_modules:
            continue
        atoms = parse_mm_file(mm_path)
        if atoms:
            all_atoms.extend(atoms)
            by_module[module] = atoms

    if args.report or args.dry_run:
        print(generate_report(all_atoms))
        print()

    if not by_module:
        print("No trusted atoms found in std/", file=sys.stderr)
        sys.exit(1)

    if not args.dry_run:
        args.output.mkdir(parents=True, exist_ok=True)

    for module, atoms in sorted(by_module.items()):
        content = generate_module_file(module, atoms)
        out_path = args.output / f"ffi_{module}.rs"
        if args.dry_run:
            print(f"--- {out_path} ---")
            print(content)
        else:
            out_path.write_text(content, encoding="utf-8")
            print(f"Generated: {out_path}")

    # Generate mod.rs that re-exports all test modules
    if not args.dry_run:
        mod_content = (
            "//! Auto-generated module index for FFI contract tests.\n"
            "//!\n"
            "//! Generated by scripts/ffi_contract_test_gen.py\n\n"
        )
        for module in sorted(by_module):
            mod_content += f"mod ffi_{module};\n"
        mod_path = args.output / "mod.rs"
        mod_path.write_text(mod_content, encoding="utf-8")
        print(f"Generated: {mod_path}")

    if args.report:
        print()
        print("Done.")


if __name__ == "__main__":
    main()
