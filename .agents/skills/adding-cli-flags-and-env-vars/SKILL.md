---
name: adding-cli-flags-and-env-vars
description: "Use when adding, renaming, or removing a CLI flag, subcommand option, or environment variable"
---

# Adding CLI flags and environment variables

1. Read `docs/NAMING.md` and choose a name that follows its rules. Do not rename an existing name in place; add an alias and a deprecation note.
2. Add the CLI flag to the clap command definition. For `mumei verify`, set an appropriate `help_heading`.
3. Add each environment variable to `src/env_registry.rs` with its category, default, description, and `sensitive` classification.
4. Regenerate the committed CLI and environment references:
   ```sh
   MUMEI_UPDATE_REFERENCE_DOCS=1 cargo test --bin mumei reference_docs
   ```
5. Run the naming, reference, and config tests:
   ```sh
   cargo test --bin mumei naming
   cargo test --bin mumei cli_reference
   cargo test --test config_cli
   ```
6. Check `cargo run -- config --all` to confirm the variable is reported correctly. For a sensitive variable, use a fake sentinel and confirm its value is absent from both `cargo run -- config` and `cargo run -- config --json` output.

## PR checklist

- [ ] The new name follows `docs/NAMING.md`; an existing name remains available through an alias and deprecation note.
- [ ] Environment metadata includes category, default, description, and an explicit sensitive-value decision.
- [ ] CLI and environment references are regenerated and the naming, reference, and config tests pass.
- [ ] `config` output reports the setting correctly without printing any sensitive sentinel.
