# Agent guidelines

These rules are for coding agents and harnesses that work in this repository (Devin, Codex, Claude Code, Cursor and others). [`.claude/CLAUDE.md`](.claude/CLAUDE.md) covers the MCP workflow and `.mm` syntax. The rules below apply in addition to it.

## Fast path

Build, lint and test locally before opening a PR. The toolchain needs LLVM 17 and Z3:

```bash
export LLVM_SYS_170_PREFIX=/usr/lib/llvm-17
export LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu

cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace --lib --bins
cargo run -- verify std/contracts.mm --json   # smoke-test the verifier
python -m pytest tests/test_mcp_server.py -v  # needs: pip install pytest -r requirements-mcp.txt
```

For the full CI gate, also run the integration tests that `--lib --bins` skips (the two Lean-bridge skips match `core-tests.yml`, which doesn't check out mumei-lean):

```bash
cargo test --workspace --exclude mumei-ffi-tests --test '*' --no-fail-fast -- \
  --skip escalate_lean_promotes_outside_fragment_to_candidate_bundle \
  --skip nlsat_first_fixture_outside_window_keeps_lean_escalation
```

The pre-commit hooks run the same checks; install them once with `pre-commit install`. Note the hooks *fix* files rather than check them: `cargo fmt` rewrites sources in place, and the Rust hooks only run when the staged change includes Rust files.

Known gotchas:

- `cargo build`/`cargo run` fail without `LLVM_SYS_170_PREFIX` when LLVM is not on the default path.
- Narrow test runs are fine (`cargo test -p mumei-core <name>`); run the full workspace before pushing.
- GitHub Actions workflows under `.github/workflows/` gate on path filters, so a docs-only PR may legitimately show no Rust checks.

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
- Never rename an existing name in place; add an alias and deprecate the old name; remove it only at 1.0 (after 1.0, at the next major release).
- Register every environment variable in `src/env_registry.rs`, including category, default, description, and `sensitive` classification; regenerate references with `MUMEI_UPDATE_REFERENCE_DOCS=1 cargo test --bin mumei reference_docs`.
- Run `cargo test --bin mumei naming`, `cargo test --bin mumei cli_reference`, and `cargo test --test config_cli`.
- Use the [adding CLI flags and environment variables skill](.agents/skills/adding-cli-flags-and-env-vars/SKILL.md).

## Agent skills layout

Two skill directories exist and they serve different audiences:

- [`.github/skills/`](.github/skills/README.md) — the public "Mumei Forge" primitive catalog (verify, build, diagnose, certify, catalog). These skills ship with the repository for agents *using* the toolchain, and are composed by the orchestration agent described in that README.
- [`.agents/skills/`](.agents/skills/) — contributor-facing skills for working *on* this repository (testing recipes, authoring guides). They are internal and not part of the public catalog.
