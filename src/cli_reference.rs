use clap::{Arg, Command as ClapCommand, CommandFactory};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::cli::Cli;

#[derive(Clone, Copy)]
struct EnvVar {
    name: &'static str,
    category: &'static str,
    default: &'static str,
    description: &'static str,
}

const ENV_VARS: &[EnvVar] = &[
    EnvVar { name: "MUMEI_ENABLE_VACUITY_CHECK", category: "user", default: "off", description: "Enables the build pipeline's vacuity checks when set to a truthy value." },
    EnvVar { name: "MUMEI_LEAN_PATH", category: "user", default: "auto-detected", description: "Path to a mumei-lean checkout used for Lean escalation; the bridge script is resolved beneath this path." },
    EnvVar { name: "MUMEI_PROOF_STORE", category: "user", default: "$HOME/.mumei/proof-store", description: "Selects the local proof-store directory. Setting this variable does not enable proof-store use." },
    EnvVar { name: "MUMEI_PROOF_BUNDLE", category: "integration", default: "unset", description: "Fallback proof-certificate bundle used when resolving imported modules." },
    EnvVar { name: "MUMEI_REGISTRY_ALLOW_PLAINTEXT", category: "user", default: "off", description: "Explicitly permits a configured remote package registry to use plaintext HTTP." },
    EnvVar { name: "MUMEI_REGISTRY_URL", category: "user", default: "unset", description: "Overrides the remote package registry URL from mumei.toml when non-empty." },
    EnvVar { name: "MUMEI_STD_PATH", category: "user", default: "unset", description: "Additional directory searched for Mumei standard-library modules." },
    EnvVar { name: "MUMEI_CANCEL_REASON", category: "integration", default: "unset", description: "Cancellation reason included in generated proof-certificate provenance." },
    EnvVar { name: "MUMEI_GENERATION_ID", category: "integration", default: "unset", description: "Generation identifier included in proof-certificate provenance." },
    EnvVar { name: "MUMEI_SELF_CORRECTION_METADATA", category: "integration", default: "unset", description: "JSON metadata from self-correction included in proof-certificate provenance." },
    EnvVar { name: "MUMEI_SOLVER_CONFIG_FINGERPRINT", category: "integration", default: "computed", description: "Optional explicit solver-configuration fingerprint for provenance and cache identity." },
    EnvVar { name: "MUMEI_SPEC_METADATA", category: "integration", default: "unset", description: "Specification metadata attached to verification results." },
    EnvVar { name: "MUMEI_TASK_ID", category: "integration", default: "unset", description: "Task identifier propagated to verification and proof-certificate provenance." },
    EnvVar { name: "MUMEI_TRACE_ID", category: "integration", default: "unset", description: "Trace identifier attached to verification metadata." },
    EnvVar { name: "MUMEI_VERIFICATION_TIMEOUT_MS", category: "integration", default: "unset", description: "Timeout in milliseconds propagated to verification workers and proof certificates." },
    EnvVar { name: "MUMEI_SOLVER_CACHE_KEY", category: "integration", default: "computed", description: "Optional explicit solver-cache key recorded in proof-certificate provenance." },
    EnvVar { name: "MUMEI_SOLVER_PROCESS_START_TIME", category: "integration", default: "unset", description: "Solver process start timestamp included in proof-certificate provenance." },
    EnvVar { name: "MUMEI_HARNESS_CONTRACT", category: "integration", default: "unset", description: "Harness contract path recorded in proof-certificate provenance." },
    EnvVar { name: "MUMEI_INTENT_PROMPT_HASH", category: "integration", default: "unset", description: "Natural-language intent prompt hash recorded in proof-certificate provenance." },
    EnvVar { name: "MUMEI_SPEC_TRACEABILITY_SCORE", category: "integration", default: "unset", description: "Specification traceability score recorded in proof-certificate provenance." },
    EnvVar { name: "MUMEI_SEMANTIC_DRIFT_DETECTED", category: "integration", default: "unset", description: "Semantic-drift result recorded in proof-certificate provenance." },
    EnvVar { name: "MUMEI_MANUAL_REVIEW_REQUIRED", category: "integration", default: "unset", description: "Manual-review requirement recorded in proof-certificate provenance." },
    EnvVar { name: "MUMEI_ARTIFACT_PATHS", category: "integration", default: "unset", description: "Comma-separated artifact paths recorded in proof-certificate provenance." },
    EnvVar { name: "MUMEI_BUDGET_POLICY_FINGERPRINT", category: "integration", default: "unset", description: "Budget-policy fingerprint recorded in proof-certificate provenance." },
    EnvVar { name: "OTEL_ENABLED", category: "integration", default: "off", description: "Enables OpenTelemetry tracing when set to a truthy value." },
    EnvVar { name: "OTEL_EXPORTER_OTLP_ENDPOINT", category: "integration", default: "OpenTelemetry SDK default", description: "OTLP endpoint used by the tracing exporter when telemetry is enabled." },
    EnvVar { name: "TRACEPARENT", category: "integration", default: "unset", description: "W3C trace context propagated into verification spans." },
    EnvVar { name: "TRACESTATE", category: "integration", default: "unset", description: "W3C trace state propagated with TRACEPARENT." },
    EnvVar { name: "MUMEI_HOME", category: "install", default: "$HOME/.mumei", description: "Installation directory selected by scripts/install.sh." },
    EnvVar { name: "MUMEI_INSTALL_EXPECTED_SHA256", category: "install", default: "unset", description: "Expected installer download SHA-256; when set, the downloaded artifact is verified before use." },
    EnvVar { name: "MUMEI_SETUP_EXPECTED_SHA256", category: "install", default: "unset", description: "Expected setup archive SHA-256; when set, the downloaded archive is verified before use." },
    EnvVar { name: "CPATH", category: "build", default: "existing value or unset", description: "C compiler include search paths; setup instructions prepend Z3 headers." },
    EnvVar { name: "CPPFLAGS", category: "build", default: "existing value or unset", description: "C preprocessor flags; setup instructions add LLVM and Z3 include directories." },
    EnvVar { name: "DYLD_FALLBACK_LIBRARY_PATH", category: "build", default: "existing value or unset", description: "macOS dynamic-library search path emitted by setup instructions for Z3." },
    EnvVar { name: "LD_LIBRARY_PATH", category: "build", default: "existing value or unset", description: "Dynamic-library search path emitted by setup instructions for Z3 on Linux." },
    EnvVar { name: "LDFLAGS", category: "build", default: "existing value or unset", description: "Linker flags; setup instructions add LLVM library directories." },
    EnvVar { name: "LIBRARY_PATH", category: "build", default: "existing value or unset", description: "Native linker library search path emitted by setup instructions for Z3." },
    EnvVar { name: "LLVM_SYS_170_PREFIX", category: "build", default: "system LLVM discovery", description: "LLVM 17 installation prefix used by llvm-sys and the native linker." },
    EnvVar { name: "LIBCLANG_PATH", category: "build", default: "system libclang discovery", description: "libclang library directory passed through to nested Cargo builds that use bindgen." },
    EnvVar { name: "Z3_SYS_Z3_HEADER", category: "build", default: "system Z3 discovery", description: "Path to z3.h used by z3-sys when compiling against a custom Z3 installation." },
    EnvVar { name: "Z3_SYS_Z3_LIB_DIR", category: "build", default: "system Z3 discovery", description: "Library directory used by z3-sys when linking a custom Z3 installation." },
    EnvVar { name: "MUMEI_UPDATE_REFERENCE_DOCS", category: "development", default: "off", description: "Set to 1 when running the reference-docs test to regenerate the committed CLI and environment reference pages." },
];

const GENERATED_HEADER: &str = "<!-- Generated by `MUMEI_UPDATE_REFERENCE_DOCS=1 cargo test --bin mumei reference_docs`; do not edit by hand. -->";

pub(crate) fn render_cli_reference() -> String {
    let mut command = Cli::command();
    command.build();
    let mut commands = Vec::new();
    collect_commands(&mut command, String::new(), &mut commands);

    let mut output = String::from(
        "---\nlayout: default\ntitle: \"CLI Option Reference — Mumei\"\ndescription: \"Generated exhaustive reference for visible Mumei commands, arguments, options, defaults, and accepted values.\"\n---\n\n",
    );
    output.push_str(GENERATED_HEADER);
    output.push_str("\n\n# Mumei CLI Option Reference\n\n");
    output.push_str("| Command | Option count | Summary |\n|---|---:|---|\n");
    for (path, about, option_count) in &commands {
        output.push_str(&format!(
            "| `mumei{}` | {} | {} |\n",
            if path.is_empty() {
                String::new()
            } else {
                format!(" {path}")
            },
            option_count,
            markdown_cell(about)
        ));
    }
    for (path, _, _) in &commands {
        let mut command = Cli::command();
        command.build();
        if let Some(target) = find_command_mut(&mut command, path) {
            render_command_section(target, path, &mut output);
        }
    }
    output
}

fn collect_commands(
    command: &mut ClapCommand,
    path: String,
    rows: &mut Vec<(String, String, usize)>,
) {
    if command.is_hide_set() || command.get_name() == "help" {
        return;
    }
    let about = command
        .get_long_about()
        .or_else(|| command.get_about())
        .map(ToString::to_string)
        .unwrap_or_else(|| "—".to_owned());
    let option_count = visible_arguments(command)
        .filter(|arg| !arg.is_positional())
        .count();
    rows.push((path.clone(), about, option_count));
    let child_names = command
        .get_subcommands()
        .filter(|child| !child.is_hide_set() && child.get_name() != "help")
        .map(|child| child.get_name().to_owned())
        .collect::<Vec<_>>();
    for name in child_names {
        let child_path = if path.is_empty() {
            name.clone()
        } else {
            format!("{path} {name}")
        };
        if let Some(child) = command.find_subcommand_mut(&name) {
            collect_commands(child, child_path, rows);
        }
    }
}

fn find_command_mut<'a>(command: &'a mut ClapCommand, path: &str) -> Option<&'a mut ClapCommand> {
    if path.is_empty() {
        return Some(command);
    }
    let mut current = command;
    for component in path.split_whitespace() {
        current = current.find_subcommand_mut(component)?;
    }
    Some(current)
}

fn visible_arguments(command: &ClapCommand) -> impl Iterator<Item = &Arg> {
    command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set() && !matches!(arg.get_id().as_str(), "help" | "version"))
}

fn render_command_section(command: &mut ClapCommand, path: &str, output: &mut String) {
    let title = if path.is_empty() {
        "mumei".to_owned()
    } else {
        format!("mumei {path}")
    };
    output.push_str(&format!("\n## `{title}`\n\n"));
    if let Some(about) = command
        .get_long_about()
        .or_else(|| command.get_about())
        .map(ToString::to_string)
    {
        let about = markdown_text(&about);
        if !about.is_empty() {
            output.push_str(&format!("{about}\n\n"));
        }
    }
    output.push_str("Usage:\n\n```text\n");
    output.push_str(&command.render_usage().to_string());
    output.push_str("\n```\n");

    let positionals = visible_arguments(command)
        .filter(|arg| arg.is_positional())
        .collect::<Vec<_>>();
    if !positionals.is_empty() {
        output.push_str(
            "\n### Positional arguments\n\n| Name | Required | Description |\n|---|---|---|\n",
        );
        for arg in positionals {
            let name = arg
                .get_value_names()
                .and_then(|names| names.first())
                .map(ToString::to_string)
                .unwrap_or_else(|| arg.get_id().as_str().to_ascii_uppercase());
            let required = if arg.is_required_set() { "Yes" } else { "No" };
            let description = arg
                .get_long_help()
                .or_else(|| arg.get_help())
                .map(ToString::to_string)
                .unwrap_or_else(|| "—".to_owned());
            output.push_str(&format!(
                "| `{}` | {} | {} |\n",
                markdown_cell(&name),
                required,
                markdown_cell(&description)
            ));
        }
        output.push('\n');
    }

    let options = visible_arguments(command)
        .filter(|arg| !arg.is_positional())
        .collect::<Vec<_>>();
    if !options.is_empty() {
        output.push_str("### Options\n\n| Option | Default | Possible values | Description |\n|---|---|---|---|\n");
        for arg in options {
            let name = option_name(arg);
            let defaults = arg
                .get_default_values()
                .iter()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(", ");
            let possible_values = if arg.get_action().takes_values() {
                arg.get_value_parser()
                    .possible_values()
                    .map(|values| {
                        values
                            .filter(|value| !value.is_hide_set())
                            .map(|value| value.get_name().to_owned())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|values| !values.is_empty())
                    .unwrap_or_else(|| "—".to_owned())
            } else {
                "—".to_owned()
            };
            let description = arg
                .get_long_help()
                .or_else(|| arg.get_help())
                .map(ToString::to_string)
                .unwrap_or_else(|| "—".to_owned());
            output.push_str(&format!(
                "| `{}` | {} | {} | {} |\n",
                markdown_cell(&name),
                markdown_cell(if defaults.is_empty() {
                    "—"
                } else {
                    &defaults
                }),
                markdown_cell(&possible_values),
                markdown_cell(&description)
            ));
        }
    }
}

fn option_name(arg: &Arg) -> String {
    let mut parts = Vec::new();
    if let Some(short) = arg.get_short() {
        parts.push(format!("-{short}"));
    }
    if let Some(long) = arg.get_long() {
        parts.push(format!("--{long}"));
    }
    let mut name = if parts.is_empty() {
        arg.get_id().as_str().to_owned()
    } else {
        parts.join(", ")
    };
    if arg.get_action().takes_values() {
        let value = arg
            .get_value_names()
            .and_then(|names| names.first())
            .map(ToString::to_string)
            .unwrap_or_else(|| arg.get_id().as_str().to_ascii_uppercase());
        name.push_str(&format!(" <{value}>"));
    }
    name
}

fn markdown_cell(value: &str) -> String {
    markdown_text(value).replace('|', "\\|")
}

fn markdown_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn render_env_reference() -> String {
    let mut output = String::from(
        "---\nlayout: default\ntitle: \"Environment Variable Reference — Mumei\"\ndescription: \"Generated reference for Mumei user configuration, integrations, installation, and build environment variables.\"\n---\n\n",
    );
    output.push_str(GENERATED_HEADER);
    output.push_str("\n\n# Mumei Environment Variable Reference\n\n");
    output.push_str("Variables are grouped by how they are used. Integration and build variables are generally supplied by the surrounding toolchain rather than set manually.\n");
    for category in ["user", "integration", "install", "build", "development"] {
        output.push_str(&format!(
            "\n## {}\n\n| Variable | Default | Description |\n|---|---|---|\n",
            category_title(category)
        ));
        for var in ENV_VARS.iter().filter(|var| var.category == category) {
            output.push_str(&format!(
                "| `{}` | {} | {} |\n",
                var.name,
                markdown_cell(var.default),
                markdown_cell(var.description)
            ));
        }
    }
    output
}

fn category_title(category: &str) -> &'static str {
    match category {
        "user" => "User configuration",
        "integration" => "Integration and trace propagation",
        "install" => "Installation and setup",
        "build" => "Build and linker configuration",
        _ => "Development",
    }
}

fn rust_env_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r#"(?s)(?:std\s*::\s*)?env\s*::\s*var(?:_os)?\s*\(\s*"([A-Za-z_][A-Za-z0-9_]*)""#,
        )
        .expect("valid Rust environment-variable scanner")
    })
}

fn literal_rust_env_reads(source: &str) -> Vec<String> {
    rust_env_pattern()
        .captures_iter(source)
        .map(|capture| capture[1].to_owned())
        .collect()
}

fn workspace_source_dirs(root: &Path) -> Vec<PathBuf> {
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read workspace manifest");
    let parsed: toml::Value = toml::from_str(&manifest).expect("parse workspace manifest");
    let mut dirs = vec![root.join("src")];
    if let Some(members) = parsed
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
    {
        for member in members.iter().filter_map(toml::Value::as_str) {
            let source_dir = root.join(member).join("src");
            if source_dir.is_dir() {
                dirs.push(source_dir);
            }
        }
    }
    dirs
}

fn rust_source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in workspace_source_dirs(root) {
        collect_rust_files(&dir, &mut files);
    }
    files
}

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn scanned_sources(root: &Path) -> Vec<PathBuf> {
    let mut files = rust_source_files(root);
    let scripts = root.join("scripts");
    let install = scripts.join("install.sh");
    if install.is_file() {
        files.push(install);
    }
    if let Ok(entries) = fs::read_dir(&scripts) {
        files.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("setup"))
        }));
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn reference_docs_are_current_or_regenerated() {
        let root = root();
        let references = [
            ("docs/CLI_REFERENCE.md", render_cli_reference()),
            ("docs/ENV_REFERENCE.md", render_env_reference()),
        ];
        for (relative, rendered) in references {
            let path = root.join(relative);
            if std::env::var("MUMEI_UPDATE_REFERENCE_DOCS").as_deref() == Ok("1") {
                fs::write(&path, rendered).expect("write generated reference");
            } else {
                let committed = fs::read_to_string(&path).unwrap_or_default();
                assert_eq!(
                    committed,
                    rendered,
                    "{relative} is stale; regenerate with MUMEI_UPDATE_REFERENCE_DOCS=1 cargo test --bin mumei reference_docs"
                );
            }
        }
    }

    #[test]
    fn environment_registry_covers_workspace_reads_and_scripts_without_stale_entries() {
        let root = root();
        let files = scanned_sources(&root);
        let mut reads = BTreeSet::new();
        let mut all_source = String::new();
        let mumei_name_pattern = Regex::new(r"MUMEI_[A-Z0-9_]+").unwrap();
        for path in &files {
            let source = fs::read_to_string(path).expect("read scanned source");
            if path.extension().is_some_and(|extension| extension == "rs") {
                reads.extend(literal_rust_env_reads(&source));
            } else {
                reads.extend(
                    mumei_name_pattern
                        .find_iter(&source)
                        .map(|capture| capture.as_str().to_owned()),
                );
            }
            if path
                .file_name()
                .is_some_and(|name| name != "cli_reference.rs")
            {
                all_source.push_str(&source);
            } else if let Some(start) = source.find("const ENV_VARS:") {
                let registry_end = source[start..]
                    .find("\n];")
                    .map(|offset| start + offset + 3)
                    .unwrap_or(start);
                all_source.push_str(&source[..start]);
                all_source.push_str(&source[registry_end..]);
            }
        }
        let registry = ENV_VARS.iter().map(|var| var.name).collect::<BTreeSet<_>>();
        let allowed_platform = BTreeSet::from(["CARGO_MANIFEST_DIR", "HOME"]);
        let missing = reads
            .difference(&registry.iter().map(|name| (*name).to_owned()).collect())
            .filter(|name| !allowed_platform.contains(name.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "unregistered environment reads: {missing:?}"
        );

        let stale = ENV_VARS
            .iter()
            .filter(|var| !all_source.contains(var.name))
            .map(|var| var.name)
            .collect::<Vec<_>>();
        assert!(
            stale.is_empty(),
            "unreferenced environment registry entries: {stale:?}"
        );
    }

    #[test]
    fn registry_names_and_categories_are_unique_and_valid() {
        let mut names = BTreeSet::new();
        for var in ENV_VARS {
            assert!(
                names.insert(var.name),
                "duplicate registry entry: {}",
                var.name
            );
            assert!(
                ["user", "integration", "install", "build", "development"].contains(&var.category)
            );
        }
    }

    #[test]
    fn markdown_cells_escape_pipes_and_collapse_newlines() {
        assert_eq!(markdown_cell("left | right\nnext"), "left \\| right next");
    }

    #[test]
    fn scanner_finds_single_line_multiline_var_os_and_comment_mentions() {
        let source = [
            format!("{}{:?});", "env::var(", "ONE"),
            format!("{}{}({:?});", "std :: env :: var_os", "\n", "TWO"),
            format!(
                "{}{:?}) is intentionally counted.",
                "// env::var(", "COMMENTED"
            ),
        ]
        .join("\n");
        assert_eq!(literal_rust_env_reads(&source), ["ONE", "TWO", "COMMENTED"]);
    }

    #[test]
    fn renderer_includes_nested_commands_defaults_possible_values_and_skips_hidden_entries() {
        let mut fixture = ClapCommand::new("mumei")
            .arg(Arg::new("hidden-option").long("hidden-option").hide(true))
            .subcommand(
                ClapCommand::new("visible")
                    .arg(
                        Arg::new("mode")
                            .long("mode")
                            .default_value("fast")
                            .value_parser(["fast", "safe"]),
                    )
                    .subcommand(ClapCommand::new("nested"))
                    .subcommand(ClapCommand::new("secret").hide(true)),
            );
        fixture.build();
        let mut output = String::new();
        let mut rows = Vec::new();
        collect_commands(&mut fixture, String::new(), &mut rows);
        for (path, _, _) in rows {
            if let Some(command) = find_command_mut(&mut fixture, &path) {
                render_command_section(command, &path, &mut output);
            }
        }
        assert!(output.contains("## `mumei visible nested`"));
        assert!(output.contains("`--mode <MODE>` | fast | fast, safe"));
        assert!(!output.contains("hidden-option"));
        assert!(!output.contains("mumei visible secret"));
    }

    #[test]
    fn every_visible_cli_subcommand_has_a_section_and_verify_lists_all_options() {
        let rendered = render_cli_reference();
        let mut command = Cli::command();
        command.build();
        let mut rows = Vec::new();
        collect_commands(&mut command, String::new(), &mut rows);
        for (path, _, _) in &rows {
            if !path.is_empty() {
                assert!(rendered.contains(&format!("## `mumei {path}`")));
            }
        }
        let verify = rows.iter().find(|(path, _, _)| path == "verify").unwrap();
        let section_start = rendered.find("## `mumei verify`").unwrap();
        let section_end = rendered[section_start + 1..]
            .find("\n## `")
            .map(|offset| section_start + 1 + offset)
            .unwrap_or(rendered.len());
        let section = &rendered[section_start..section_end];
        let listed = section
            .lines()
            .filter(|line| line.starts_with("| `--") || line.starts_with("| `-"))
            .count();
        assert_eq!(listed, verify.2);
        assert!(rendered.contains("proof-store prune"));
        assert!(rendered.contains("charter check"));
    }

    #[test]
    fn output_is_deterministic_and_contains_no_terminal_ansi() {
        let first = render_cli_reference();
        assert_eq!(first, render_cli_reference());
        assert!(!first.contains('\u{1b}'));
    }
}
