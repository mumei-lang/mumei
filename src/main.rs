#![allow(clippy::result_large_err)]

mod agent;
mod cli;
mod codegen;
mod commands;
mod feedback;
mod linker;
mod lsp;
mod pipeline;
mod setup;
mod telemetry;

use clap::Parser;
use cli::{Cli, Command};

fn main() {
    miette::set_hook(Box::new(|_| {
        Box::new(
            miette::GraphicalReportHandler::new().with_theme(miette::GraphicalTheme::unicode()),
        )
    }))
    .ok();

    telemetry::init_telemetry();

    let cli = Cli::parse();

    match cli.command {
        Some(command @ Command::Build { .. }) => commands::build::cmd_build_command(command),
        Some(command @ Command::Verify { .. }) => commands::verify::cmd_verify_command(command),
        Some(Command::Check { input }) => commands::check::cmd_check(&input),
        Some(Command::Init { name }) => commands::init::cmd_init(&name),
        Some(Command::Inspect { input, ai, format }) => {
            if let Some(ref file) = input {
                commands::inspect::cmd_inspect_file(file, ai, &format);
            } else {
                commands::inspect::cmd_inspect();
            }
        }
        Some(Command::Setup { force }) => setup::run(force),
        Some(Command::Add {
            dep,
            version,
            emitter,
            path,
            force,
        }) => match (emitter, dep) {
            (Some(name), _) => commands::add::cmd_add_emitter(&name, path.as_deref(), force),
            (None, Some(dep)) => {
                // clap's `requires = "emitter"` does not fire once the
                // positional dependency is present, so reject the emitter-only
                // flags here instead of dropping them silently.
                if path.is_some() || force {
                    eprintln!(
                        "❌ Error: --path and --force apply to `mumei add --emitter <name>` only."
                    );
                    std::process::exit(1);
                }
                commands::add::cmd_add(&dep, version.as_deref())
            }
            (None, None) => {
                eprintln!("Usage: mumei add <dep> | mumei add --emitter <name> [--path <path>]");
                std::process::exit(1);
            }
        },
        Some(Command::Publish {
            proof_only,
            allow_lean_verified,
        }) => commands::publish::cmd_publish(proof_only, allow_lean_verified),
        Some(Command::List) => commands::list::cmd_list(),
        Some(Command::Lsp) => lsp::run(),
        Some(Command::Repl) => commands::repl::cmd_repl(),
        Some(Command::Doc {
            input,
            output,
            format,
        }) => commands::doc::cmd_doc(&input, &output, &format),
        Some(Command::InferEffects { input }) => commands::infer::cmd_infer_effects(&input),
        Some(Command::InferContracts { input }) => commands::infer::cmd_infer_contracts(&input),
        Some(Command::VerifyCert {
            cert,
            input,
            allow_lean_verified,
            strict,
        }) => commands::verify_cert::cmd_verify_cert(&cert, &input, allow_lean_verified, strict),
        Some(Command::Run {
            input,
            emit,
            output,
            allow_lean_verified,
            args,
        }) => commands::run::cmd_run(&input, &emit, output.as_deref(), allow_lean_verified, &args),
        None => {
            if let Some(ref input) = cli.input {
                commands::build::cmd_build_default(input, &cli.output);
            } else {
                eprintln!("Usage: mumei <COMMAND> or mumei <input.mm>");
                eprintln!("  build   Verify + compile (default)");
                eprintln!("  verify  Z3 formal verification only");
                eprintln!("  check   Parse + resolve only (fast syntax check)");
                eprintln!("  run     Build and run a mumei program as a native binary");
                eprintln!("  init    Generate a new project template");
                eprintln!("  setup   Download & configure Z3 + LLVM toolchain");
                eprintln!("  add     Add a dependency to mumei.toml, or install an emitter plugin (--emitter)");
                eprintln!("  lsp     Start Language Server Protocol server");
                eprintln!("  repl    Interactive REPL (Read-Eval-Print Loop)");
                eprintln!("  doc     Generate documentation from source comments");
                eprintln!("  inspect Inspect development environment");
                eprintln!("Run `mumei --help` for full usage.");
                std::process::exit(1);
            }
        }
    }

    telemetry::shutdown_telemetry();
}
