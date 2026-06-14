use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "mumei",
    version = env!("CARGO_PKG_VERSION"),
    about = "🗡 Mumei — Mathematical Proof-Driven Programming Language",
    long_about = "Formally verified language: parse → resolve → monomorphize → verify (Z3) → codegen (LLVM IR)"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,

    /// Input .mm file (backward compat: `mumei input.mm` = `mumei build input.mm`)
    #[arg(global = false)]
    pub(crate) input: Option<String>,

    /// Output base name (for .ll)
    #[arg(short, long, default_value = "katana")]
    pub(crate) output: String,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Command {
    /// Verify + compile to LLVM IR (default)
    Build {
        /// Input .mm file
        input: String,
        /// Output base name
        #[arg(short, long, default_value = "katana")]
        output: String,
        /// Emit target: llvm-ir (default), c-header, verified-json, decidable-metrics, proof-book, proof-cert, escalation-bundle
        #[arg(long, default_value = "llvm-ir")]
        emit: String,
        /// P5-C: Strict import mode — missing/invalid certificates cause hard errors
        #[arg(long)]
        strict_imports: bool,
        /// PR 2: Accept mumei-lean-emitted certificates
        /// (`z3_check_result == "lean_verified"`) as proven during import
        /// resolution. Off by default for backwards compatibility — the resolver
        /// only trusts Z3-discharged (`unsat`) atoms unless this flag is set.
        #[arg(long)]
        allow_lean_verified: bool,
    },
    /// Z3 formal verification only (no codegen)
    Verify {
        /// Input .mm file or directory
        input: String,
        /// Verification task ID for MCP/CI provenance
        #[arg(long)]
        task_id: Option<String>,
        /// Override Z3 solver timeout in milliseconds
        #[arg(long)]
        solver_timeout: Option<u64>,
        /// Verification cache scope: module (input directory) or global (current workspace)
        #[arg(long, default_value = "module", value_parser = ["module", "global"])]
        cache_scope: String,
        /// Generate Z3 proof certificate (.proof.json)
        #[arg(long)]
        proof_cert: bool,
        /// Escalate Z3 unknown obligations to Lean 4 via mumei-lean bridge.
        #[arg(long)]
        escalate_lean: bool,
        /// Emit format: "escalation-bundle" writes .escalation-bundle.json
        #[arg(long, value_name = "FORMAT")]
        emit: Option<String>,
        /// Disable verify-only output targets: escalation-metrics
        #[arg(long = "no-emit")]
        no_emit: Vec<String>,
        /// Output path for proof certificate (default: <input>.proof.json)
        #[arg(long)]
        output: Option<String>,
        /// Directory to write report.json into (default: current directory)
        #[arg(long)]
        report_dir: Option<String>,
        /// Output verification report as JSON to stdout
        #[arg(long)]
        json: bool,
        /// P5-C: Strict import mode — missing/invalid certificates cause hard errors
        #[arg(long)]
        strict_imports: bool,
        /// PR 2: Accept mumei-lean-emitted certificates
        /// (`z3_check_result == "lean_verified"`) as proven during import
        /// resolution. Off by default for backwards compatibility.
        #[arg(long)]
        allow_lean_verified: bool,
        /// Enable cross-specification consistency verification across atoms
        #[arg(long)]
        cross_spec_verify: bool,
        /// Additional .mm files to include in cross-specification verification
        #[arg(long, value_delimiter = ',')]
        cross_spec_files: Vec<String>,
        /// Enable P8-A spurious counterexample detection
        #[arg(long, conflicts_with = "disable_spurious_detection")]
        enable_spurious_detection: bool,
        /// Disable P8-A spurious counterexample detection
        #[arg(long)]
        disable_spurious_detection: bool,
        /// Run property-based validation synthesized from refinement types
        #[arg(long)]
        property_based_test: bool,
        /// Emit outside_decidable_fragment warnings for atoms outside the Z3-stable fragment
        #[arg(long)]
        warn_fragment: bool,
        /// Number of generated property-based inputs per atom
        #[arg(long, default_value_t = 100)]
        property_based_test_count: usize,
        /// Seed for deterministic property-based input generation
        #[arg(long)]
        property_based_test_seed: Option<u64>,
        /// Maximum property-based shrinking steps per counterexample
        #[arg(long, default_value_t = 64)]
        property_based_test_max_shrink_steps: usize,
        /// Harness contract path or identifier to embed in generated proof certificates
        #[arg(long)]
        harness_contract: Option<String>,
        /// Intent fidelity metadata JSON to embed in generated proof certificates
        #[arg(long)]
        intent_fidelity: Option<String>,
        /// Comma-separated artifact paths to embed in generated proof certificates
        #[arg(long)]
        artifact_paths: Option<String>,
        /// Budget policy fingerprint to embed in generated proof certificates
        #[arg(long)]
        budget_policy_fingerprint: Option<String>,
        /// Emit a contract manifest containing per-atom specification hashes
        #[arg(long)]
        emit_contract_manifest: bool,
        /// Enable spec vacuity checking via mutation testing
        #[arg(long)]
        enable_vacuity_check: bool,
        /// Detect loops that may need stronger invariants
        #[arg(long)]
        detect_loops: bool,
        /// Include CEGIS loop-invariant suggestions in JSON/report output
        #[arg(long, requires = "detect_loops")]
        suggest_cegis: bool,
        /// Path to an old proof certificate for spec drift detection
        #[arg(long)]
        detect_spec_drift: Option<String>,
    },
    /// Parse + resolve + monomorphize only (no Z3, fast syntax check)
    Check {
        /// Input .mm file or directory
        input: String,
    },
    /// Generate a new Mumei project template
    Init {
        /// Project directory name
        name: String,
    },
    /// Inspect development environment (Z3, LLVM, std library)
    Inspect {
        /// Input .mm file for structured report (optional)
        input: Option<String>,
        /// Output as structured JSON for AI agents
        #[arg(long)]
        ai: bool,
        /// Output format: json or text (default: text)
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Download and configure Z3 + LLVM toolchain into ~/.mumei/
    Setup {
        /// Force re-download even if already installed
        #[arg(long)]
        force: bool,
    },
    /// Add a dependency to mumei.toml
    Add {
        /// Dependency specifier: local path (./path/to/lib) or package name
        dep: String,
        /// P5-B: Specify version for registry dependency
        #[arg(long)]
        version: Option<String>,
    },
    /// Publish package to local registry (~/.mumei/packages/)
    Publish {
        /// Publish only the proof cache (no source code)
        #[arg(long)]
        proof_only: bool,
        /// Accept mumei-lean-emitted certificates (`lean_verified`) as proven.
        #[arg(long)]
        allow_lean_verified: bool,
    },
    /// List available packages in the local registry
    List,
    /// Start Language Server Protocol server (stdio mode)
    Lsp,
    /// Interactive REPL (Read-Eval-Print Loop)
    Repl,
    /// Generate documentation from source comments
    Doc {
        /// Input .mm file or directory
        input: String,
        /// Output directory for generated docs
        #[arg(short, long, default_value = "docs_out")]
        output: String,
        /// Output format: html or markdown
        #[arg(long, default_value = "html")]
        format: String,
    },
    /// Infer required effects for all atoms (JSON output, for MCP integration)
    InferEffects {
        /// Input .mm file or directory
        input: String,
    },
    /// Infer contracts (requires/ensures) for all atoms (JSON output, Plan 13)
    InferContracts {
        /// Input .mm file or directory
        input: String,
    },
    /// Verify a proof certificate against current source
    VerifyCert {
        /// Proof certificate file (.proof.json)
        cert: String,
        /// Source .mm file to verify against
        input: String,
        /// PR 2: Accept mumei-lean-emitted certificates
        /// (`z3_check_result == "lean_verified"`) as proven. Off by default —
        /// only Z3-discharged (`unsat`) atoms are accepted.
        #[arg(long)]
        allow_lean_verified: bool,
    },
    /// P7-B: Build and run a mumei program as a native binary
    Run {
        /// Input .mm file
        input: String,
        /// Emit target: binary (default) or llvm-ir
        #[arg(long, default_value = "binary", value_parser = ["binary", "llvm-ir"])]
        emit: String,
        /// Output executable path (default: temporary binary)
        #[arg(short, long)]
        output: Option<String>,
        /// Accept mumei-lean-emitted certificates (`lean_verified`) as proven.
        #[arg(long)]
        allow_lean_verified: bool,
        /// Arguments to pass to the compiled program
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
}
