# Naming convention for flags and environment variables

This convention prevents new naming drift; it does not rename existing interfaces.
Rule IDs are enforced by the test harness in [`src/naming.rs`](../src/naming.rs).

## CLI flags

- **F1 — kebab-case:** Long flags match `--[a-z0-9]+(-[a-z0-9]+)*`; this keeps shell usage predictable. Good: `--strict-imports`. Bad: `--Strict_Imports`.
- **F2 — paired enable/disable:** `--disable-X` has a `--enable-X` in the same command; a matched pair makes both directions explicit. Good: `--enable-spurious-detection` / `--disable-spurious-detection`. Bad: `--disable-cache` alone.
- **F3 — paired no-option:** `--no-X` has `--X` in the same command; callers can clearly select either behavior. Good: `--emit` / `--no-emit`. Bad: `--no-cache` alone.
- **F4 — safety-only allow:** Every `--allow-X` is listed in `SAFETY_RELAXATIONS` with the check it relaxes; `allow` must not disguise an ordinary feature opt-in. Good: `--allow-lean-verified` with its certificate-provenance reason. Bad: `--allow-preview-mode`.
- **F5 — avoid ambiguous prefixes:** Do not start new flags with `--with-`, `--without-`, `--skip-`, `--use-`, or `--dont-`; use a direct action or a paired enable/disable name. Good: `--enable-cache`. Bad: `--skip-cache`.

## Environment variables

- **E1 — upper snake case:** Names match `[A-Z][A-Z0-9_]*`; this matches platform and shell conventions. Good: `MUMEI_REPO`. Bad: `MumeiRepo`.
- **E2 — positive feature toggles:** Repo-owned names avoid `USE_`, `DISABLE_`, `NO_`, `MUMEI_USE_`, `MUMEI_DISABLE_`, and `MUMEI_NO_`, and avoid `_ENABLED` / `_DISABLED`; positive toggles have one clear direction. Good: `MUMEI_ENABLE_CACHE`. Bad: `MUMEI_DISABLE_CACHE`.
- **E3 — repository prefix:** Repo-owned Mumei variables start with `MUMEI_`; this distinguishes them from process and toolchain settings. Good: `MUMEI_ENABLE_CACHE`. Bad: `ENABLE_CACHE`.
- **E4 — safety-only allow:** Every variable containing `ALLOW` is listed in `SAFETY_RELAXATIONS` with the check it relaxes; an allow variable is a reviewed safety exception. Good: `MUMEI_REGISTRY_ALLOW_PLAINTEXT` with its transport-policy reason. Bad: `MUMEI_ALLOW_PREVIEW`.

Build-tool, OpenTelemetry, GitHub, shared CI-fixture, OpenAI-client, and W3C trace-context variables are external and listed in `EXTERNAL`; the prefix and toggle rules do not apply to them. Mumei feature toggles use `MUMEI_ENABLE_X`.

## Adding an exception

Prefer renaming a new name so it follows the convention. Add an exception only when compatibility requires it: include the exact name, violated rule ID, and a one-line reason in `GRANDFATHERED`; document the reason and the alias/deprecation plan. The tests reject unlisted violations and stale exceptions.

## Known inversions

Mumei has no flag with the same meaning as mumei-agent's `--allow-contract-erosion`; `--allow-lean-verified` only relaxes certificate-provenance acceptance. The grandfathered E3 names in the test harness do not form an opposite-direction pair.

## Cross-layer consistency

Use the same concept stem across layers: `--enable-cache`, `MUMEI_ENABLE_CACHE`, and `cache` in a configuration file. Use `--allow-*` only to relax a safety check. Default-on features use a paired `--disable-X` or `--no-X`. Never rename an existing flag or variable in place; add an alias and keep the old name with a deprecation note until the next major release.
