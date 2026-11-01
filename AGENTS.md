# Agent guidelines

These rules are for coding agents and harnesses that work in this repository (Devin, Codex, Claude Code, Cursor and others). [`.claude/CLAUDE.md`](.claude/CLAUDE.md) covers the MCP workflow and `.mm` syntax. The rules below apply in addition to it.

## Secrets and sensitive data

These rules follow the OWASP Logging and Secrets Management cheat sheets, CWE-200, CWE-532 and CWE-798.

- Never write a secret value to any output. That includes:
  - CLI text and JSON
  - logs, OpenTelemetry spans and attributes
  - proof certificates, reports and other artifacts
  - error messages and panics
  - test snapshots, PR descriptions and commit messages

  Report only whether the secret is set.
- Don't mask secrets with `***` or by showing the first or last characters. Leave the value out entirely. A mask still says something about the value, and masking rules are easy to get wrong.
- Classify every environment variable that can hold a credential as `sensitive: true` in [`src/env_registry.rs`](src/env_registry.rs). The registry test fails when a name looks like a credential (TOKEN, KEY, SECRET, PASSWORD, AUTH, ...) but is neither classified as sensitive nor listed as a reviewed exception, so each new variable gets an explicit decision.
- Print URL values (for example `MUMEI_REGISTRY_URL` and `OTEL_EXPORTER_OTLP_ENDPOINT`) without userinfo, query or fragment, because those parts can carry credentials. `mumei config` already does this.
- Only print environment variables that are in the registry. Never dump the whole environment (`env`, `printenv`, `std::env::vars()`), including in debug output and CI logs.
- Don't commit credentials. `.env` and `.env.*` are gitignored. Use placeholders in examples and docs.
- Tests that touch secret handling use an obviously fake sentinel value and assert that it never appears in stdout or stderr.

## When you change an output surface

When a change adds or widens an output surface, check it against the rules above before opening the PR. Output surfaces include a command, a report field, a log line, an artifact and a JSON schema. Don't paste secret values into chat, issues, PRs or tool arguments that may be logged. To check a credential, check whether it is set.

## Adding or renaming flags and environment variables

- Read [`docs/NAMING.md`](docs/NAMING.md) before choosing a flag or environment-variable name.
- Never rename an existing name in place; add an alias and deprecate the old name through the next major release.
- Register every environment variable in `src/env_registry.rs`, including category, default, description, and `sensitive` classification; regenerate references with `MUMEI_UPDATE_REFERENCE_DOCS=1 cargo test --bin mumei reference_docs`.
- Run `cargo test --bin mumei naming`, `cargo test --bin mumei cli_reference`, and `cargo test --test config_cli`.
- Use the [adding CLI flags and environment variables skill](.agents/skills/adding-cli-flags-and-env-vars/SKILL.md).
