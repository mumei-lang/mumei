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
    /// Check common Evidence representation or produce fresh scoped source Evidence; no proof acceptance
    Evidence {
        #[command(subcommand)]
        command: EvidenceCommand,
    },
    /// Build/check a restricted native identity bundle (not artifact semantic proof)
    NativeBundle {
        #[command(subcommand)]
        command: NativeBundleCommand,
    },
    /// Verify + compile to LLVM IR (default)
    Build {
        /// Input .mm file
        input: String,
        /// Output base name
        #[arg(short, long, default_value = "katana")]
        output: String,
        /// Emit target: llvm-ir (default), c-header, verified-json, decidable-metrics, proof-book, proof-cert, escalation-bundle, rust-wrapper, python-wrapper, runtime-monitor
        #[arg(long, default_value = "llvm-ir")]
        emit: String,
        /// Write a <stem>.semantic-contract.json sidecar (C03 pure-i64 profile; hash binding only, correspondence unchecked)
        #[arg(long)]
        semantic_contract: bool,
        /// P5-C: Strict import mode — missing/invalid certificates cause hard errors
        #[arg(long)]
        strict_imports: bool,
        /// PR 2: Accept mumei-lean-emitted certificates
        /// (`z3_check_result == "lean_verified"`) as proven during import
        /// resolution. Off by default for backwards compatibility — the resolver
        /// only trusts Z3-discharged (`unsat`) atoms unless this flag is set.
        #[arg(long)]
        allow_lean_verified: bool,
        /// Reuse Z3-verified proofs from the opt-in local proof store.
        #[arg(long)]
        proof_store: bool,
    },
    /// Z3 formal verification only (no codegen)
    Verify {
        /// Input .mm file or directory
        input: String,
        /// Opt-in preset that turns on a group of verify checks (strict, audit); explicit flags still apply and a profile never turns a flag off
        #[arg(
            long,
            value_name = "PROFILE",
            value_parser = ["strict", "audit"],
            help_heading = "Profile and strictness"
        )]
        profile: Option<String>,
        /// P5-C: Strict import mode — missing/invalid certificates cause hard errors
        #[arg(long, help_heading = "Profile and strictness")]
        strict_imports: bool,
        /// Opt-in strict mode: treat unannotated array accesses (which default
        /// to the `i64` element sort) as errors instead of warnings. Off by
        /// default, preserving the backward-compatible `i64` fallback.
        #[arg(long, help_heading = "Profile and strictness")]
        strict_array_types: bool,
        /// Emit outside_decidable_fragment warnings for atoms outside the Z3-stable fragment
        #[arg(long, help_heading = "Profile and strictness")]
        warn_fragment: bool,
        /// Emit `untyped_array_access` warnings when an array is accessed as
        /// `arr[i]` without an explicit `[i64]`/`[f64]`/`[bool]` element-type
        /// annotation (which silently falls back to `i64`). Warning-only; does
        /// not change the verification encoding.
        #[arg(long, help_heading = "Profile and strictness")]
        warn_untyped_arrays: bool,

        /// Override Z3 solver timeout in milliseconds
        #[arg(long, help_heading = "Solver and proof")]
        solver_timeout: Option<u64>,
        /// Verification cache scope: module (input directory) or global (current workspace)
        #[arg(
            long,
            default_value = "module",
            value_parser = ["module", "global"],
            help_heading = "Solver and proof"
        )]
        cache_scope: String,
        /// Reuse Z3-verified proofs from the opt-in local proof store.
        #[arg(long, help_heading = "Solver and proof")]
        proof_store: bool,
        /// Escalate Z3 unknown obligations to Lean 4 via mumei-lean bridge.
        #[arg(long, help_heading = "Solver and proof")]
        escalate_lean: bool,
        /// PR 2: Accept mumei-lean-emitted certificates
        /// (`z3_check_result == "lean_verified"`) as proven during import
        /// resolution. Off by default for backwards compatibility.
        #[arg(long, help_heading = "Solver and proof")]
        allow_lean_verified: bool,
        /// Generate Z3 proof certificate (.proof.json)
        #[arg(long, help_heading = "Solver and proof")]
        proof_cert: bool,
        /// Opt-in IEEE 754 (binary64) verification for `f64`: encode `f64` as
        /// Z3 `Float(11,53)` with round-nearest-even FP arithmetic instead of
        /// the default exact-rational `Real` encoding. Models rounding so e.g.
        /// `0.1 + 0.2 != 0.3` holds. Default off (keeps `Real` semantics).
        #[arg(long, help_heading = "Solver and proof")]
        ieee754_f64: bool,
        /// Opt-in bit-vector verification for `i64`: encode `i64` as Z3
        /// `BV(64)` so `&`, `|`, `^`, `<<`, `>>` have real bit semantics and
        /// `+`, `-`, `*` wrap in two's complement like machine arithmetic,
        /// instead of the default unbounded `Int` encoding. Default off
        /// (keeps `Int` semantics and existing proof certificates).
        #[arg(long, help_heading = "Solver and proof")]
        bitvec_i64: bool,
        /// Opt-in fixed-width check under the default `Int` encoding: reject a
        /// body `+`, `-` or `*` whose `i64` operands give a result outside the
        /// `i64` range, where compiled code would wrap or panic. Contracts keep
        /// unbounded arithmetic. Default off.
        #[arg(long, help_heading = "Solver and proof")]
        check_i64_overflow: bool,

        /// Enable P8-A spurious counterexample detection
        #[arg(
            long,
            conflicts_with = "disable_spurious_detection",
            help_heading = "Diagnostics"
        )]
        enable_spurious_detection: bool,
        /// Disable P8-A spurious counterexample detection
        #[arg(long, help_heading = "Diagnostics")]
        disable_spurious_detection: bool,
        /// Enable spec vacuity checking via mutation testing
        #[arg(long, help_heading = "Diagnostics")]
        enable_vacuity_check: bool,
        /// Detect loops that may need stronger invariants
        #[arg(long, help_heading = "Diagnostics")]
        detect_loops: bool,
        /// Include CEGIS loop-invariant suggestions in JSON/report output
        #[arg(long, requires = "detect_loops", help_heading = "Diagnostics")]
        suggest_cegis: bool,
        /// Path to an old proof certificate for spec drift detection
        #[arg(long, help_heading = "Diagnostics")]
        detect_spec_drift: Option<String>,
        /// Enable cross-specification consistency verification across atoms and emit cross_spec.json with agent_artifact_mapping
        #[arg(long, help_heading = "Diagnostics")]
        cross_spec_verify: bool,
        /// Additional .mm files to include in cross-specification verification; contract_consistency/global_invariant_conflicts/circular_dependencies map to mumei-agent missing_constraints/divergences/drift_issues
        #[arg(long, value_delimiter = ',', help_heading = "Diagnostics")]
        cross_spec_files: Vec<String>,

        /// Run property-based validation synthesized from refinement types
        #[arg(long, help_heading = "Property-based testing")]
        property_based_test: bool,
        /// Number of generated property-based inputs per atom
        #[arg(long, default_value_t = 100, help_heading = "Property-based testing")]
        property_based_test_count: usize,
        /// Seed for deterministic property-based input generation
        #[arg(long, help_heading = "Property-based testing")]
        property_based_test_seed: Option<u64>,
        /// Maximum property-based shrinking steps per counterexample
        #[arg(long, default_value_t = 64, help_heading = "Property-based testing")]
        property_based_test_max_shrink_steps: usize,

        /// Output verification report as JSON to stdout
        #[arg(long, help_heading = "Output and artifacts")]
        json: bool,
        /// Emit format: "sarif" writes report.sarif; "loss-vector" prints P9-E Loss Vector JSON; "escalation-bundle" writes .escalation-bundle.json; "proof-graph" writes proof_graph.json for the interactive visualizer
        #[arg(long, value_name = "FORMAT", help_heading = "Output and artifacts")]
        emit: Option<String>,
        /// Disable verify-only output targets: escalation-metrics
        #[arg(long = "no-emit", help_heading = "Output and artifacts")]
        no_emit: Vec<String>,
        /// Output path for proof certificate or escalation bundle
        #[arg(long, alias = "out", help_heading = "Output and artifacts")]
        output: Option<String>,
        /// Directory to write report.json into (default: current directory)
        #[arg(long, help_heading = "Output and artifacts")]
        report_dir: Option<String>,
        /// Preserve per-atom SMT queries and verification phase outcomes
        #[arg(long, value_name = "DIR", help_heading = "Output and artifacts")]
        keep_phase_artifacts: Option<String>,
        /// Emit a contract manifest containing per-atom specification hashes
        #[arg(long, help_heading = "Output and artifacts")]
        emit_contract_manifest: bool,
        /// Verification task ID for MCP/CI provenance
        #[arg(long, help_heading = "Output and artifacts")]
        task_id: Option<String>,

        /// Harness contract path or identifier to embed in generated proof certificates
        #[arg(long, help_heading = "Harness metadata")]
        harness_contract: Option<String>,
        /// Intent fidelity metadata JSON to embed in generated proof certificates
        #[arg(long, help_heading = "Harness metadata")]
        intent_fidelity: Option<String>,
        /// Comma-separated artifact paths to embed in generated proof certificates
        #[arg(long, help_heading = "Harness metadata")]
        artifact_paths: Option<String>,
        /// Budget policy fingerprint to embed in generated proof certificates
        #[arg(long, help_heading = "Harness metadata")]
        budget_policy_fingerprint: Option<String>,
    },
    /// Show effective settings and where each value comes from
    #[command(
        long_about = "Show effective settings and where each value comes from.\n\nThe CLI `--profile` is per-invocation; `mumei config` only reports the manifest source."
    )]
    Config {
        #[arg(long)]
        json: bool,
        #[arg(long)]
        all: bool,
    },
    /// Inspect or prune the local proof store
    ProofStore {
        #[command(subcommand)]
        command: ProofStoreCommand,
    },
    /// Parse + resolve + monomorphize only (no Z3, fast syntax check)
    Check {
        /// Input .mm file or directory
        input: String,
    },
    /// Compare candidate contracts with a baseline file
    Erosion {
        /// Candidate .mm file
        candidate: String,
        /// Baseline .mm file
        #[arg(long, value_name = "FILE")]
        baseline: String,
        /// Print the erosion report as JSON
        #[arg(long)]
        json: bool,
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
    /// Add a dependency to mumei.toml, or install an emitter plugin
    Add {
        /// Dependency specifier: local path (./path/to/lib) or package name
        #[arg(required_unless_present = "emitter", conflicts_with = "emitter")]
        dep: Option<String>,
        /// P5-B: Specify version for registry dependency
        #[arg(long, conflicts_with = "emitter")]
        version: Option<String>,
        /// Emitter Plugin Phase 3: install an external emitter plugin as
        /// `<name>`, making `mumei build --emit <name>` resolve it
        #[arg(long, value_name = "NAME")]
        emitter: Option<String>,
        /// Compiled cdylib to install, or a directory/cargo project
        /// containing it (default: the current directory)
        #[arg(long, value_name = "PATH", requires = "emitter")]
        path: Option<String>,
        /// Overwrite an already installed plugin with the same name
        #[arg(long, requires = "emitter")]
        force: bool,
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
        /// Treat drifted certificates as failures: exit non-zero when an atom
        /// is reported `changed` or when the certificate carries no
        /// `certificate_hash` to re-derive. Intended for CI gates that
        /// distribute certificates alongside sources.
        #[arg(long)]
        strict: bool,
    },
    /// Summarize atom evidence and its freshness in a Claim Book
    Claims {
        /// Input .mm file
        input: String,
        /// Proof certificate path (defaults to the path written by `mumei verify --proof-cert`)
        #[arg(long)]
        cert: Option<String>,
        /// Accept current Lean-verified certificate evidence
        #[arg(long)]
        allow_lean_verified: bool,
        /// Output format: json, md, or both
        #[arg(long, default_value = "both", value_parser = ["json", "md", "both"])]
        format: String,
        /// Directory for claimbook.json and CLAIMBOOK.md (default: current directory)
        #[arg(long, default_value = ".")]
        out_dir: String,
        /// Attach observations from a mumei.test-harvest/v1 file
        #[arg(long)]
        observations: Option<String>,
        /// Attach negative-control results to claims
        #[arg(long)]
        controls: bool,
        /// Maximum mutants to test per atom (requires --controls)
        #[arg(long, default_value_t = 32, requires = "controls")]
        max_mutants: usize,
        /// Fail if any checked claim has weak or inconclusive controls
        #[arg(long)]
        require_discriminating: bool,
        /// Require non-empty claims with complete dependency coverage
        #[arg(long)]
        require_complete: bool,
        /// Require every claim to meet this Seal Grade
        #[arg(long, value_parser = ["G0", "G1", "G2", "G3", "G4", "G5"])]
        require_grade: Option<String>,
        /// Require a promotable acceptance triple: <requirement-list> <evidence> <acceptance> — the gate fails closed unless the mumei.evidence-requirement-list-acceptance/v1 descriptor pins these declared inputs with verdict accepted
        #[arg(long, num_args = 3, value_names = ["REQUIREMENT_LIST", "EVIDENCE", "ACCEPTANCE"])]
        require_evidence_acceptance: Vec<String>,
    },
    /// Check whether atom contracts reject deterministic implementation mutations
    Controls {
        /// Input .mm file
        input: String,
        /// Print the structured JSON report
        #[arg(long)]
        json: bool,
        /// Maximum mutants to test per atom
        #[arg(long, default_value_t = 32)]
        max_mutants: usize,
        /// Analyze only the named atom
        #[arg(long)]
        atom: Option<String>,
    },
    /// Check harvested examples, relations, rejections, and property tests against atom contracts
    Observe {
        /// Input .mm file
        input: String,
        /// mumei.test-harvest/v1 JSON file, including property observations
        harvest: String,
        /// Print the structured JSON report
        #[arg(long)]
        json: bool,
        /// Analyze only observations for the named atom
        #[arg(long)]
        atom: Option<String>,
    },
    /// Lock or check human-owned charter contracts
    Charter {
        #[command(subcommand)]
        command: CharterCommand,
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

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum EvidenceCommand {
    /// Freshly recheck saved source Evidence with linked Z3 and caller-supplied source/budgets; not proof acceptance
    RecheckSource {
        /// One regular common Evidence JSON record (at most 65,536 bytes), not a Native envelope/certificate
        input: String,
        /// Separate regular C03 pure-i64 source (at most 65,536 bytes); saved labels/paths are never read
        #[arg(long)]
        source: String,
        /// Emit a static consistency diagnostic, not Evidence or an acceptance receipt
        #[arg(long)]
        json: bool,
        /// Solver timeout in milliseconds (1..=10000); must match the producer's explicit budget
        #[arg(long, default_value_t = mumei_core::source_evidence::MAX_TIMEOUT_MS.to_string(), allow_hyphen_values = true)]
        solver_timeout: String,
        /// Solver resource units (1..=1000000); no saved/ambient policy is loaded
        #[arg(long, default_value_t = mumei_core::source_evidence::MAX_RESOURCE_LIMIT.to_string(), allow_hyphen_values = true)]
        solver_resource_limit: String,
        /// Declared RFC 3339 timestamp the producer stamped; must equal the saved record's produced_at
        #[arg(long)]
        produced_at: Option<String>,
    },
    /// Freshly recheck a semantic-contract sidecar against caller-supplied source and LLVM IR bytes; hash binding and source-model obligations only, not LLVM correspondence or acceptance
    RecheckSidecar {
        /// Regular semantic-contract sidecar JSON (at most 4 MiB)
        sidecar: String,
        /// Separate regular source file (at most 65,536 bytes); the saved label is never opened
        #[arg(long)]
        source: String,
        /// Separate regular LLVM IR artifact (at most 16 MiB); hash-bound only
        #[arg(long)]
        artifact: String,
        /// Emit a static diagnostic, not evidence or an acceptance receipt
        #[arg(long)]
        json: bool,
        /// Solver timeout in milliseconds (1..=10000)
        #[arg(long, default_value_t = mumei_core::source_evidence::MAX_TIMEOUT_MS.to_string(), allow_hyphen_values = true)]
        solver_timeout: String,
        /// Solver resource units (1..=1000000); zero/unlimited is rejected
        #[arg(long, default_value_t = mumei_core::source_evidence::MAX_RESOURCE_LIMIT.to_string(), allow_hyphen_values = true)]
        solver_resource_limit: String,
    },
    /// Evaluate a semantic-contract sidecar's hash binding and fresh source obligations into a bounded acceptance descriptor
    AcceptSidecar {
        /// Regular semantic-contract sidecar JSON (at most 4 MiB)
        sidecar: String,
        /// Separate regular source file (at most 65,536 bytes); the saved label is never opened
        #[arg(long)]
        source: String,
        /// Separate regular LLVM IR artifact (at most 16 MiB); hash-bound only
        #[arg(long)]
        artifact: String,
        /// Emit a static acceptance descriptor, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
        /// Solver timeout in milliseconds (1..=10000)
        #[arg(long, default_value_t = mumei_core::source_evidence::MAX_TIMEOUT_MS.to_string(), allow_hyphen_values = true)]
        solver_timeout: String,
        /// Solver resource units (1..=1000000); zero/unlimited is rejected
        #[arg(long, default_value_t = mumei_core::source_evidence::MAX_RESOURCE_LIMIT.to_string(), allow_hyphen_values = true)]
        solver_resource_limit: String,
        #[command(flatten)]
        signature: SignatureArgs,
    },
    /// Evaluate representation plus fresh same-profile source consistency into a bounded acceptance descriptor
    AcceptSource {
        /// One regular common Evidence JSON record (at most 65,536 bytes), not a Native envelope/certificate
        input: String,
        /// Separate regular C03 pure-i64 source (at most 65,536 bytes); saved labels/paths are never read
        #[arg(long)]
        source: String,
        /// Emit a static acceptance descriptor, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
        /// Solver timeout in milliseconds (1..=10000); must match the producer's explicit budget
        #[arg(long, default_value_t = mumei_core::source_evidence::MAX_TIMEOUT_MS.to_string(), allow_hyphen_values = true)]
        solver_timeout: String,
        /// Solver resource units (1..=1000000); no saved/ambient policy is loaded
        #[arg(long, default_value_t = mumei_core::source_evidence::MAX_RESOURCE_LIMIT.to_string(), allow_hyphen_values = true)]
        solver_resource_limit: String,
        /// Declared RFC 3339 timestamp the producer stamped; must equal the saved record's produced_at
        #[arg(long)]
        produced_at: Option<String>,
        #[command(flatten)]
        signature: SignatureArgs,
    },
    /// Freshly verify one C03 pure-i64 source postcondition, not LLVM/artifacts or proof acceptance
    VerifySource {
        /// Regular source at most 65,536 bytes; one pure-i64 comparison/conditional atom, explicit clauses, at most 16 parameters and literal true precondition
        input: String,
        /// Emit one common Evidence v1 record on fresh unsat only (not an acceptance receipt)
        #[arg(long)]
        json: bool,
        /// Solver timeout in milliseconds (1..=10000); no project/ambient policy loaded
        #[arg(long, default_value_t = mumei_core::source_evidence::MAX_TIMEOUT_MS.to_string(), allow_hyphen_values = true)]
        solver_timeout: String,
        /// Solver resource units (1..=1000000); zero/unlimited is rejected
        #[arg(long, default_value_t = mumei_core::source_evidence::MAX_RESOURCE_LIMIT.to_string(), allow_hyphen_values = true)]
        solver_resource_limit: String,
        /// Declared RFC 3339 timestamp stamped on the emitted record; never read from the system clock
        #[arg(long)]
        produced_at: Option<String>,
    },
    /// Check read-only i64 slice source obligations with Z3; emits a JSON diagnostic, not Evidence or LLVM
    CheckReadonlySlice {
        /// One regular source file (at most 65,536 bytes) containing one shared-slice atom
        input: String,
    },
    /// Detachably sign a bounded record's raw bytes with a declared ed25519 key into a producer-signature envelope; binds the declared key id only
    Sign {
        /// Regular file (at most 4 MiB (4,194,304 bytes)) whose raw bytes become the signed subject; never parsed or canonicalized
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        input: Option<String>,
        /// Read the record bytes from standard input (at most 4 MiB (4,194,304 bytes)); never infer stdin from a missing file
        #[arg(long)]
        stdin: bool,
        /// Emit the detached mumei.producer-signature/v1 envelope, not a verification or acceptance result
        #[arg(long)]
        json: bool,
        /// Regular file holding exactly the 64-char lowercase-hex ed25519 secret key; the value is never printed
        #[arg(long)]
        key_file: String,
        /// Caller-declared producer key label the consumer pins separately; never derived from the key bytes
        #[arg(long)]
        producer_key_id: String,
        /// Optional declared production timestamp stamped into the envelope (RFC 3339, excluding leap seconds); unsigned like the key id — freshness is a consumer-side check
        #[arg(long)]
        produced_at: Option<String>,
    },
    /// Generate a fresh ed25519 producer keypair; writes the 64-char hex secret to a new file and prints only the public key
    Keygen {
        /// New regular file receiving the 64-char lowercase-hex ed25519 secret key; must not already exist
        #[arg(long)]
        secret_file: String,
    },
    /// Validate one Evidence v1 JSON record's representation, not its truth, identity, freshness or authenticity
    Check {
        /// Regular JSON file (at most 65,536 bytes); not a native report envelope or proof certificate
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        input: Option<String>,
        /// Read one common Evidence record from standard input (at most 65,536 bytes); never infer stdin from a missing file
        #[arg(long)]
        stdin: bool,
        /// Print a static representation-only diagnostic, not an accepted Evidence record or certificate
        #[arg(long)]
        json: bool,
        /// Require the record's declared producer adapter id; declared identity only, not producer authenticity
        #[arg(long)]
        expected_adapter: Option<String>,
    },
    /// Validate one agent attempt-schedule record's representation and consistency only
    #[command(
        long_about = "Core checks only the static representation and schedule consistency. A `selected` outcome names only a manifest-declared-success exit, never acceptance. `reported_outcome` is the adapter's manifest-declared self-report, never result polarity or a verdict. The optional `enforcement` object, when present, must have exactly `environment`, `network`, `filesystem_write`, `filesystem_read`, `process_spawn`, and `memory_bound_mb` keys. It is the agent's self-reported effective dispatch boundary and is checked for shape only; it is not Evidence or a resource-enforcement attestation. Manifest-declared argv, stdin and memory bounds are not attested by this record; embedded evidence_acceptance is carried, not re-judged. No adapter is executed and no grade is promoted."
    )]
    CheckSchedule {
        /// Regular mumei.attempt-schedule/v1 JSON file (at most 65,536 bytes)
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        input: Option<String>,
        /// Read one attempt-schedule record from standard input (at most 65,536 bytes)
        #[arg(long)]
        stdin: bool,
        /// Print a static representation-only diagnostic; no adapter execution or grade promotion
        #[arg(long)]
        json: bool,
    },
    /// Validate one verifier-adapter manifest's representation and consistency only
    #[command(
        long_about = "Core checks only the static representation and manifest consistency. Manifest-declared argv and stdin remain inert unless host policy opts in through allow_manifest_attempt_argv and allow_manifest_attempt_stdin; a host argv map takes precedence. Verdict markers describe an adapter self-report vocabulary, never a verdict. max_memory_mb is the adapter's declared ceiling: planning uses the smaller of it and the policy memory bound; a scheduled attempt uses it only when the host declares no memory bound. Core does not enforce it. No adapter is executed and no grade is promoted."
    )]
    CheckAdapter {
        /// Regular mumei.verifier-adapter/v1 JSON file (at most 65,536 bytes)
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        input: Option<String>,
        /// Read one adapter manifest from standard input (at most 65,536 bytes)
        #[arg(long)]
        stdin: bool,
        /// Print a static representation-only diagnostic; no adapter execution or grade promotion
        #[arg(long)]
        json: bool,
    },
    /// Validate one scheduled evidence-check acceptance representation only
    #[command(
        long_about = "Core validates the closed representation and producer consistency of a scheduled evidence-check acceptance. `accepted` records the producer's binding of a declared check outcome to its current candidate, never Core Evidence acceptance. `candidate_match` is not rechecked because Core has no attempt-sequence snapshot. No Core check is rerun and no grade is promoted."
    )]
    CheckAttemptAcceptance {
        /// Regular mumei.evidence-check-acceptance/v1 JSON file (at most 65,536 bytes)
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        input: Option<String>,
        /// Read one evidence-check acceptance from standard input (at most 65,536 bytes)
        #[arg(long)]
        stdin: bool,
        /// Print a static representation-only diagnostic; no Core check rerun or grade promotion
        #[arg(long)]
        json: bool,
    },
    /// Normalize one declared requirement against one Evidence record into a bounded verdict; no tool execution or proof acceptance
    Accept {
        /// Regular Evidence v1 JSON record file (at most 65,536 bytes); not a native report envelope or proof certificate
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        input: Option<String>,
        /// Read one common Evidence record from standard input (at most 65,536 bytes); never infer stdin from a missing file
        #[arg(long)]
        stdin: bool,
        /// Regular mumei.evidence-requirement/v1 JSON file (at most 65,536 bytes); the declared pins and assurance floor
        #[arg(
            long,
            required_unless_present = "requirement_list",
            conflicts_with = "requirement_list"
        )]
        requirement: Option<String>,
        /// Regular mumei.evidence-requirement-list/v1 JSON file (at most 65,536 bytes); a conjunctive set of declared requirements
        #[arg(long)]
        requirement_list: Option<String>,
        /// Emit a static mumei.evidence-acceptance/v1 descriptor, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        signature: SignatureArgs,
    },
    /// Normalize one declared requirement against a bounded Evidence list envelope — per-record verdicts plus an aggregate; no tool execution or proof acceptance
    AcceptList {
        /// Regular mumei.verification-evidence-list/v1 JSON file (at most 65,536 bytes); not a native report envelope or proof certificate
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        input: Option<String>,
        /// Read one bounded Evidence list envelope from standard input (at most 65,536 bytes); never infer stdin from a missing file
        #[arg(long)]
        stdin: bool,
        /// Regular mumei.evidence-requirement/v1 JSON file (at most 65,536 bytes); the declared pins and assurance floor
        #[arg(
            long,
            required_unless_present = "requirement_list",
            conflicts_with = "requirement_list"
        )]
        requirement: Option<String>,
        /// Regular mumei.evidence-requirement-list/v1 JSON file (at most 65,536 bytes); a conjunctive set of declared requirements
        #[arg(long)]
        requirement_list: Option<String>,
        /// Emit a static mumei.evidence-list-acceptance/v1 descriptor, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        signature: SignatureArgs,
    },
    /// Validate a bounded Evidence list envelope's representation — every record individually, not their truth or consistency
    CheckList {
        /// Regular JSON file (at most 65,536 bytes); not a native report envelope or proof certificate
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        input: Option<String>,
        /// Read one common Evidence list envelope from standard input (at most 65,536 bytes); never infer stdin from a missing file
        #[arg(long)]
        stdin: bool,
        /// Print a static representation-only diagnostic, not accepted Evidence or a certificate
        #[arg(long)]
        json: bool,
        /// Require every record's declared producer adapter id; declared identity only, not producer authenticity
        #[arg(long)]
        expected_adapter: Option<String>,
    },
    /// Check a bounded Evidence list envelope's cross-record identity-pin agreement, not pin correctness or truth
    CheckConsistency {
        /// Regular JSON file (at most 65,536 bytes); not a native report envelope or proof certificate
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        input: Option<String>,
        /// Read one common Evidence list envelope from standard input (at most 65,536 bytes); never infer stdin from a missing file
        #[arg(long)]
        stdin: bool,
        /// Print a static structural-consistency diagnostic, not accepted Evidence or a certificate
        #[arg(long)]
        json: bool,
        /// Require every record's declared producer adapter id; declared identity only, not producer authenticity
        #[arg(long)]
        expected_adapter: Option<String>,
    },
    /// Validate a native object-route acceptance descriptor and optionally project it into Common Evidence v1
    CheckObjectAcceptance {
        /// Regular mumei.native-evidence-acceptance/v1 object-route JSON file (at most 262,144 bytes)
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        input: Option<String>,
        /// Read one object-route acceptance descriptor from standard input (at most 262,144 bytes)
        #[arg(long)]
        stdin: bool,
        /// Emit a static representation diagnostic JSON (not a recheck or acceptance receipt)
        #[arg(long, conflicts_with = "emit_evidence")]
        json: bool,
        /// Project the validated descriptor into a bounded mumei.verification-evidence/v1 record (no recheck or grade promotion)
        #[arg(long, conflicts_with = "json")]
        emit_evidence: bool,
    },
}

#[derive(clap::Args)]
pub(crate) struct SignatureArgs {
    /// Regular mumei.producer-signature/v1 envelope file (at most 65,536 bytes)
    #[arg(long, requires = "producer_key")]
    pub(crate) signature: Option<String>,
    /// 64-char lowercase-hex ed25519 producer public key
    #[arg(long, requires = "signature")]
    pub(crate) producer_key: Option<String>,
    /// Pin the unsigned producer key id to the supplied key; required for registry/revocation checks
    #[arg(long, requires = "signature")]
    pub(crate) expected_producer_key: Option<String>,
    /// Declared RFC 3339 reference time for freshness and registry checks
    #[arg(long, requires = "signature")]
    pub(crate) reference_time: Option<String>,
    /// Maximum envelope age in seconds
    #[arg(long, requires_all = ["signature", "reference_time"])]
    pub(crate) max_age_seconds: Option<u64>,
    /// Regular mumei.producer-key-registry/v1 file (at most 65,536 bytes); requires a pinned producer key id
    #[arg(long, requires_all = ["signature", "reference_time", "expected_producer_key"])]
    pub(crate) key_registry: Option<String>,
    /// Regular mumei.producer-signature/v1 registry signature file
    #[arg(long, requires_all = ["signature", "key_registry", "registry_key"])]
    pub(crate) registry_signature: Option<String>,
    /// 64-char lowercase-hex ed25519 registry root public key
    #[arg(long, requires_all = ["signature", "key_registry", "registry_signature"])]
    pub(crate) registry_key: Option<String>,
    /// Regular mumei.key-revocation-list/v1 or /v2 file (at most 65,536 bytes); requires a pinned producer key id
    #[arg(long, requires_all = ["signature", "expected_producer_key"])]
    pub(crate) revocation_list: Option<String>,
    /// Regular mumei.producer-signature/v1 revocation-list signature file
    #[arg(long, requires_all = ["signature", "revocation_list", "revocation_key"])]
    pub(crate) revocation_signature: Option<String>,
    /// 64-char lowercase-hex ed25519 revocation root public key
    #[arg(long, requires_all = ["signature", "revocation_list", "revocation_signature"])]
    pub(crate) revocation_key: Option<String>,
    /// Minimum acceptable v2 version; checks not_after and requires a root-signed list and reference time
    #[arg(
        long,
        value_parser = clap::value_parser!(u64).range(1..),
        requires_all = [
            "signature",
            "revocation_list",
            "revocation_signature",
            "revocation_key",
            "reference_time"
        ]
    )]
    pub(crate) min_revocation_version: Option<u64>,
}

#[derive(clap::Args)]
pub(crate) struct ObjectDescriptorOutputArgs {
    /// Emit a static acceptance descriptor, not reusable Evidence or a certificate
    #[arg(long)]
    pub(crate) json: bool,
    /// Regular file containing the hex-encoded ed25519 signing key, never printed
    #[arg(long, requires = "signature_output")]
    pub(crate) key_file: Option<String>,
    /// Declared producer key label, not signed or authenticated
    #[arg(long, requires = "signature_output")]
    pub(crate) producer_key_id: Option<String>,
    /// Optional declared production timestamp stamped into the signature envelope (RFC 3339, excluding leap seconds); unsigned like the key id
    #[arg(long, requires = "signature_output")]
    pub(crate) produced_at: Option<String>,
    /// Write a detached signature envelope for the exact JSON output without replacing a file
    #[arg(long, requires_all = ["json", "key_file", "producer_key_id"])]
    pub(crate) signature_output: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum NativeBundleCommand {
    /// Evaluate a saved native consumer Evidence report against fresh pinned source/LLVM checks into a bounded acceptance descriptor
    AcceptEvidence {
        bundle: String,
        source: String,
        evidence: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel, not the report
        #[arg(long)]
        expected_build_id: String,
        /// Emit a static acceptance descriptor, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
    },
    /// Evaluate pinned artifact reproduction under the recorded toolchain into a bounded acceptance descriptor
    AcceptArtifact {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
        /// Emit a static acceptance descriptor, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
    },
    /// Evaluate reproduced object machine code against the verified source within the machine-code TV profile into a bounded acceptance descriptor
    AcceptObject {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
        #[command(flatten)]
        output: ObjectDescriptorOutputArgs,
    },
    /// Replay a saved native consumer Evidence report against fresh pinned source/LLVM checks, without proof acceptance
    RecheckEvidence {
        bundle: String,
        source: String,
        evidence: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel, not the report
        #[arg(long)]
        expected_build_id: String,
    },
    /// Replay a saved accept-object descriptor against a fresh object acceptance, optionally pinning its detached producer signature
    RecheckObjectAcceptance {
        bundle: String,
        source: String,
        descriptor: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel, not the descriptor
        #[arg(long)]
        expected_build_id: String,
        #[command(flatten)]
        signature: Box<SignatureArgs>,
        /// Emit a machine-readable mumei.native-object-acceptance-recheck/v1 descriptor instead of text
        #[arg(long)]
        json: bool,
    },
    /// Recheck the C03 source and pinned native LLVM body/wrapper snapshot, not object or binary semantics
    RecheckLlvm {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
        /// Print scoped common Evidence JSON on success, without proof acceptance or grade promotion
        #[arg(long)]
        json: bool,
    },
    /// Check pinned snapshots and freshly recheck the C03 source postcondition, not LLVM or binary semantics
    RecheckSource {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
    },
    /// Freshly verify and build a pure-i64 main on Linux/x86_64 with fixed settings
    Build {
        input: String,
        /// New bundle directory; must not exist (project config and proof cache are not loaded)
        #[arg(short, long, default_value = "native-bundle")]
        output: String,
    },
    /// Check snapshot identities against an externally supplied build ID; does not execute or prove the artifact
    Check {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
    },
    /// Recheck that the toolchain reproduces the pinned object and executable; does not execute or check runtime/ABI semantics
    RecheckArtifact {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
    },
    /// Recheck object symbols, direct call edges and linked function placement/call targets; no instruction semantics, ABI or runtime claim
    RecheckObject {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
        /// Emit a mumei.object-symbol-correspondence/v1 descriptor on success, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
    },
    /// Compare straight-line and acyclic-branch machine code, including conditional direct calls, with pure-i64 source models under assumed register conventions; no ABI or runtime claim
    RecheckMachineTv {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
        /// Emit a mumei.machine-code-tv/v1 descriptor on success, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
    },
    /// Execute the reproduced executable and compare its OS exit status with the record pin; no runtime/ABI semantics
    RecheckExitStatus {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
    },
    /// Accept the pinned artifact after the exit-status recheck; emits an anonymous acceptance descriptor, not reusable Evidence
    AcceptExitStatus {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
        /// Emit a static acceptance descriptor, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
    },
    /// Execute the reproduced executable and compare its captured stdout digest with the record pin; no runtime/ABI semantics
    RecheckStdout {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
    },
    /// Accept the pinned artifact after the stdout recheck; emits an anonymous acceptance descriptor, not reusable Evidence
    AcceptStdout {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
        /// Emit a static acceptance descriptor, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
    },
    /// Execute the reproduced executable and compare its captured stderr digest with the record pin; no runtime/ABI semantics
    RecheckStderr {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
    },
    /// Accept the pinned artifact after the stderr recheck; emits an anonymous acceptance descriptor, not reusable Evidence
    AcceptStderr {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
        /// Emit a static acceptance descriptor, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
    },
    /// Execute the reproduced executable once and compare its exit status, stdout and stderr with every record pin; no runtime/ABI semantics
    RecheckObservations {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
        /// Emit a mumei.runtime-observation/v1 descriptor on a matching observation, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
    },
    /// Accept the artifact chain plus one invocation of the reproduced executable matching every runtime-observation pin; no runtime/ABI semantics
    AcceptObservations {
        bundle: String,
        source: String,
        /// SHA-256 of native-bundle.json obtained through a separately trusted channel
        #[arg(long)]
        expected_build_id: String,
        /// Emit a bounded acceptance descriptor on an accepted outcome, not reusable Evidence or a certificate
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum ProofStoreCommand {
    /// Show local proof-store entry counts and size
    Stats {
        #[arg(long)]
        dir: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Remove old and/or invalid proof-store entries
    Prune {
        #[arg(long)]
        dir: Option<String>,
        #[arg(long)]
        older_than_days: Option<u64>,
        #[arg(long)]
        invalid: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum CharterCommand {
    /// Write deterministic hashes for the selected charter files
    Lock {
        /// Charter file path; may be repeated
        #[arg(long = "charter", action = clap::ArgAction::Append)]
        charter: Vec<String>,
        /// Lock file path
        #[arg(long)]
        lock: Option<String>,
    },
    /// Check an implementation against the locked charter contracts
    Check {
        /// Implementation .mm file
        input: String,
        /// Charter file path; may be repeated
        #[arg(long = "charter", action = clap::ArgAction::Append)]
        charter: Vec<String>,
        /// Lock file path
        #[arg(long)]
        lock: Option<String>,
        /// Proof certificate for the implementation
        #[arg(long)]
        cert: Option<String>,
        /// Minimum effective Seal Grade required for each chartered atom
        #[arg(long, value_parser = ["G0", "G1", "G2", "G3", "G4", "G5"])]
        require_grade: Option<String>,
        /// Accept current Lean-verified certificate evidence
        #[arg(long)]
        allow_lean_verified: bool,
        /// Print the JSON report to standard output
        #[arg(long)]
        json: bool,
    },
}
