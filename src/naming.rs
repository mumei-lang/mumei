use std::collections::BTreeSet;

use clap::{ArgAction, Command as ClapCommand, CommandFactory};

use crate::cli::Cli;
use crate::env_registry::ENV_VARS;

const GRANDFATHERED: &[(&str, &str, &str)] = &[
    (
        "ENABLE_RECONSTRUCTION_LOSS",
        "E3",
        "pre-convention name; renaming needs an alias and a deprecation window (see docs/NAMING.md)",
    ),
    (
        "ENABLE_SELF_CORRECTION",
        "E3",
        "pre-convention name; renaming needs an alias and a deprecation window (see docs/NAMING.md)",
    ),
];

const SAFETY_RELAXATIONS: &[(&str, &str)] = &[
    (
        "--allow-lean-verified",
        "Relaxes certificate provenance checks by accepting certificates marked lean_verified.",
    ),
    (
        "MUMEI_REGISTRY_ALLOW_PLAINTEXT",
        "Relaxes registry transport policy by allowing non-loopback HTTP URLs.",
    ),
];

const EXTERNAL: &[(&str, &str)] = &[
    (
        "category:build",
        "Compiler and linker variables are consumed by external build tooling.",
    ),
    (
        "OTEL_*",
        "OpenTelemetry variables are defined by the tracing SDK and ecosystem.",
    ),
    ("TRACEPARENT", "W3C distributed trace context variable."),
    ("TRACESTATE", "W3C distributed trace state variable."),
    (
        "GITHUB_*",
        "Variables defined by GitHub Actions and GitHub tooling.",
    ),
    (
        "OPENAI_API_KEY",
        "Credential name defined by the OpenAI client ecosystem.",
    ),
    (
        "CI_FIXTURE_MODE",
        "Shared fixture-mode control used by repository CI.",
    ),
    (
        "LD_LIBRARY_PATH",
        "Dynamic loader variable defined by the operating system.",
    ),
    (
        "LLVM_SYS_170_PREFIX",
        "Installation prefix defined by llvm-sys tooling.",
    ),
    ("Z3_SYS_*", "Build variables defined by z3-sys tooling."),
];

fn collect_flags(
    command: &ClapCommand,
    path: &mut Vec<String>,
    flags: &mut BTreeSet<(String, String)>,
) {
    path.push(command.get_name().to_owned());
    let command_path = path.join(" ");
    for argument in command.get_arguments() {
        if matches!(argument.get_action(), ArgAction::Help | ArgAction::Version) {
            continue;
        }
        if let Some(long) = argument.get_long() {
            flags.insert((command_path.clone(), format!("--{long}")));
        }
    }
    for subcommand in command.get_subcommands() {
        collect_flags(subcommand, path, flags);
    }
    path.pop();
}

fn command_flags() -> BTreeSet<(String, String)> {
    let mut command = Cli::command();
    command.build();
    let mut flags = BTreeSet::new();
    collect_flags(&command, &mut Vec::new(), &mut flags);
    flags
}

fn is_kebab_flag(flag: &str) -> bool {
    let Some(name) = flag.strip_prefix("--") else {
        return false;
    };
    !name.is_empty()
        && name.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn flag_violations(flags: &BTreeSet<(String, String)>) -> BTreeSet<(String, String)> {
    let mut violations = BTreeSet::new();
    for (command, flag) in flags {
        let scoped = format!("{command} {flag}");
        if !is_kebab_flag(flag) {
            violations.insert((scoped.clone(), "F1".to_owned()));
        }
        if let Some(suffix) = flag.strip_prefix("--disable-") {
            let counterpart = (command.clone(), format!("--enable-{suffix}"));
            if !flags.contains(&counterpart) {
                violations.insert((scoped.clone(), "F2".to_owned()));
            }
        }
        if let Some(suffix) = flag.strip_prefix("--no-") {
            let counterpart = (command.clone(), format!("--{suffix}"));
            if !flags.contains(&counterpart) {
                violations.insert((scoped.clone(), "F3".to_owned()));
            }
        }
        if flag.starts_with("--allow-") && !SAFETY_RELAXATIONS.iter().any(|(name, _)| *name == flag)
        {
            violations.insert((scoped.clone(), "F4".to_owned()));
        }
        if ["--with-", "--without-", "--skip-", "--use-", "--dont-"]
            .iter()
            .any(|prefix| flag.starts_with(prefix))
        {
            violations.insert((scoped, "F5".to_owned()));
        }
    }
    violations
}

fn is_external(name: &str, category: &str) -> bool {
    EXTERNAL.iter().any(|(selector, _)| {
        if let Some(expected_category) = selector.strip_prefix("category:") {
            category == expected_category
        } else if let Some(prefix) = selector.strip_suffix('*') {
            name.starts_with(prefix)
        } else {
            name == *selector
        }
    })
}

fn is_upper_snake(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z'))
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

fn environment_violations(variables: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    let mut violations = BTreeSet::new();
    for (name, category) in variables {
        if !is_upper_snake(name) {
            violations.insert(((*name).to_owned(), "E1".to_owned()));
        }
        if name.contains("ALLOW")
            && !SAFETY_RELAXATIONS
                .iter()
                .any(|(safety_name, _)| *safety_name == *name)
        {
            violations.insert(((*name).to_owned(), "E4".to_owned()));
        }
        if is_external(name, category) {
            continue;
        }
        if [
            "USE_",
            "DISABLE_",
            "NO_",
            "MUMEI_USE_",
            "MUMEI_DISABLE_",
            "MUMEI_NO_",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
            || name.ends_with("_ENABLED")
            || name.ends_with("_DISABLED")
        {
            violations.insert(((*name).to_owned(), "E2".to_owned()));
        }
        if !name.starts_with("MUMEI_") {
            violations.insert(((*name).to_owned(), "E3".to_owned()));
        }
    }
    violations
}

fn current_environment() -> Vec<(&'static str, &'static str)> {
    ENV_VARS
        .iter()
        .map(|variable| (variable.name, variable.category))
        .collect()
}

fn all_violations() -> BTreeSet<(String, String)> {
    let mut violations = flag_violations(&command_flags());
    violations.extend(environment_violations(&current_environment()));
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(entries: &[(&str, &str)]) -> BTreeSet<(String, String)> {
        entries
            .iter()
            .map(|(command, flag)| ((*command).to_owned(), (*flag).to_owned()))
            .collect()
    }

    fn assert_flag_rule(entries: &[(&str, &str)], name: &str, rule: &str) {
        let violations = flag_violations(&flags(entries));
        assert!(
            violations.contains(&(name.to_owned(), rule.to_owned())),
            "{rule} regression was not rejected; see docs/NAMING.md"
        );
    }

    fn assert_env_rule(entries: &[(&str, &str)], name: &str, rule: &str) {
        let violations = environment_violations(entries);
        assert!(
            violations.contains(&(name.to_owned(), rule.to_owned())),
            "{rule} regression was not rejected; see docs/NAMING.md"
        );
    }

    #[test]
    fn existing_names_only_break_recorded_rules() {
        let actual = all_violations();
        let grandfathered = GRANDFATHERED
            .iter()
            .map(|(name, rule, _)| ((*name).to_owned(), (*rule).to_owned()))
            .collect::<BTreeSet<_>>();
        let unrecorded = actual
            .difference(&grandfathered)
            .cloned()
            .collect::<Vec<_>>();
        let stale = grandfathered
            .difference(&actual)
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            unrecorded.is_empty(),
            "unrecorded naming violations: {unrecorded:?}; see docs/NAMING.md"
        );
        assert!(
            stale.is_empty(),
            "stale grandfathered names: {stale:?}; see docs/NAMING.md"
        );
    }

    #[test]
    fn every_safety_relaxation_is_present() {
        let flags = command_flags();
        let flag_names = flags
            .iter()
            .map(|(_, flag)| flag.as_str())
            .collect::<BTreeSet<_>>();
        let variables = current_environment();
        let env_names = variables
            .iter()
            .map(|(name, _)| *name)
            .collect::<BTreeSet<_>>();
        for (name, reason) in SAFETY_RELAXATIONS {
            assert!(
                !reason.is_empty(),
                "safety reason is empty; see docs/NAMING.md"
            );
            assert!(
                flag_names.contains(*name) || env_names.contains(*name),
                "stale safety relaxation {name}; see docs/NAMING.md"
            );
        }
    }

    #[test]
    fn flag_rule_regressions_are_rejected_and_compliant_names_are_accepted() {
        assert_flag_rule(
            &[("mumei demo", "--Bad_Name")],
            "mumei demo --Bad_Name",
            "F1",
        );
        assert!(is_kebab_flag("--good-name"), "F1: see docs/NAMING.md");

        assert_flag_rule(
            &[("mumei demo", "--disable-feature")],
            "mumei demo --disable-feature",
            "F2",
        );
        assert!(
            flag_violations(&flags(&[
                ("mumei demo", "--disable-feature"),
                ("mumei demo", "--enable-feature"),
            ]))
            .is_empty(),
            "F2 compliant pair rejected; see docs/NAMING.md"
        );

        assert_flag_rule(
            &[("mumei demo", "--no-cache")],
            "mumei demo --no-cache",
            "F3",
        );
        assert!(
            flag_violations(&flags(&[
                ("mumei demo", "--no-cache"),
                ("mumei demo", "--cache"),
            ]))
            .is_empty(),
            "F3 compliant pair rejected; see docs/NAMING.md"
        );

        assert_flag_rule(
            &[("mumei demo", "--allow-unsafe-mode")],
            "mumei demo --allow-unsafe-mode",
            "F4",
        );
        assert!(
            flag_violations(&flags(&[("mumei demo", "--allow-lean-verified")]))
                .iter()
                .all(|(_, rule)| rule != "F4"),
            "F4 documented safety relaxation rejected; see docs/NAMING.md"
        );

        assert_flag_rule(
            &[("mumei demo", "--skip-cache")],
            "mumei demo --skip-cache",
            "F5",
        );
        assert!(
            flag_violations(&flags(&[("mumei demo", "--good-cache")])).is_empty(),
            "F5 compliant name rejected; see docs/NAMING.md"
        );
    }

    #[test]
    fn environment_rule_regressions_are_rejected_and_compliant_names_are_accepted() {
        assert_env_rule(&[("mumei_repo", "user")], "mumei_repo", "E1");
        assert!(
            environment_violations(&[("MUMEI_REPO", "user")]).is_empty(),
            "E1 compliant name rejected; see docs/NAMING.md"
        );

        assert_env_rule(
            &[("MUMEI_DISABLE_CACHE", "user")],
            "MUMEI_DISABLE_CACHE",
            "E2",
        );
        assert_env_rule(
            &[("MUMEI_CACHE_ENABLED", "user")],
            "MUMEI_CACHE_ENABLED",
            "E2",
        );
        assert!(
            environment_violations(&[("MUMEI_ENABLE_CACHE", "user")]).is_empty(),
            "E2 compliant name rejected; see docs/NAMING.md"
        );

        assert_env_rule(&[("ENABLE_CACHE", "user")], "ENABLE_CACHE", "E3");
        assert!(
            environment_violations(&[("MUMEI_ENABLE_CACHE", "user")]).is_empty(),
            "E3 compliant name rejected; see docs/NAMING.md"
        );

        assert_env_rule(
            &[("MUMEI_ALLOW_BYPASS", "user")],
            "MUMEI_ALLOW_BYPASS",
            "E4",
        );
        assert!(
            environment_violations(&[("MUMEI_REGISTRY_ALLOW_PLAINTEXT", "user")])
                .iter()
                .all(|(_, rule)| rule != "E4"),
            "E4 documented safety relaxation rejected; see docs/NAMING.md"
        );
    }
}
