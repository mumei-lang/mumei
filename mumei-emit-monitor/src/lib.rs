//! Proof-aware runtime monitor generator (P23 Proof-Aware Observability).
//!
//! **NOT a transpiler.** This emitter generates lightweight Rust guards that
//! wrap a compiled mumei atom and report contract violations as OpenTelemetry
//! events instead of panicking.
//!
//! The defining property is what it does *not* generate: an atom whose proof
//! is self-contained (fully verified, no `extern` backing, no `effect_pre`
//! assumption) produces **no artifact at all**, so proven code stays
//! zero-cost. Only trust boundaries — see
//! [`mumei_core::trust_boundary`] — are instrumented.
//!
//! The generated code carries no dependency of its own: reporting goes through
//! a hook that the host application installs (typically wiring it to its
//! existing OTel SDK). Without `OTEL_ENABLED` the monitor is a no-op, and the
//! default hook targets `OTEL_EXPORTER_OTLP_ENDPOINT`.

use mumei_core::contract_host::{
    contract_text_to_host, quantifier_source_text, quantifier_to_host, ContractVars, HostTarget,
};
use mumei_core::emitter::{Artifact, ArtifactKind, EmitAtom, Emitter};
use mumei_core::hir::{HirBinOp, HirClauseKind, HirExpr, HirStmt};
use mumei_core::lowering::{lower, LoweredType};
use mumei_core::parser::ExternBlock;
use mumei_core::verification::{ModuleEnv, MumeiResult};
use std::path::Path;

/// Runtime support code shared by every generated monitor.
const MONITOR_RUNTIME: &str = r#"
/// Runtime support for mumei proof-aware monitors.
///
/// Reporting is a no-op unless `OTEL_ENABLED` is truthy. When enabled, the
/// violation is forwarded to the hook installed via `set_violation_hook`
/// (wire this to your OpenTelemetry SDK); the default hook writes to stderr
/// and names the configured `OTEL_EXPORTER_OTLP_ENDPOINT`.
pub mod mumei_monitor {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    /// A single observed contract violation at a trust boundary.
    #[derive(Debug, Clone)]
    pub struct Violation {
        pub atom: &'static str,
        pub boundary: &'static str,
        pub contract: &'static str,
        pub expression: &'static str,
        /// Effect state the host reported, for `effect_pre` violations.
        pub observed: Option<String>,
    }

    type Hook = fn(&Violation);
    /// Reports the effect state the host currently observes, if it tracks one.
    type EffectStateProbe = fn(&str) -> Option<String>;

    static HOOK: OnceLock<Hook> = OnceLock::new();
    static PROBE: OnceLock<EffectStateProbe> = OnceLock::new();
    static ENABLED: OnceLock<bool> = OnceLock::new();
    static WARNED: AtomicBool = AtomicBool::new(false);

    /// Install the OTel reporting hook. Call once during startup.
    pub fn set_violation_hook(hook: Hook) -> Result<(), &'static str> {
        HOOK.set(hook).map_err(|_| "violation hook already installed")
    }

    /// Install the effect-state probe. Without it the runtime effect state is
    /// unobservable, so `effect_pre` assumptions are left unchecked.
    pub fn set_effect_state_probe(probe: EffectStateProbe) -> Result<(), &'static str> {
        PROBE
            .set(probe)
            .map_err(|_| "effect state probe already installed")
    }

    /// The host's current state for `effect`, or `None` when untracked.
    ///
    /// A faulty probe must not unwind through monitored code, so a panicking
    /// probe is treated as "state unobservable".
    pub fn observed_effect_state(effect: &str) -> Option<String> {
        PROBE.get().and_then(|probe| {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| probe(effect))) {
                Ok(state) => state,
                Err(_) => {
                    eprintln!("mumei.monitor.probe_panicked effect={}", effect);
                    None
                }
            }
        })
    }

    /// `true` when `OTEL_ENABLED` is truthy; otherwise monitors are no-ops.
    pub fn enabled() -> bool {
        *ENABLED.get_or_init(|| {
            matches!(
                std::env::var("OTEL_ENABLED")
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
    }

    /// OTLP endpoint the default hook reports against.
    pub fn endpoint() -> String {
        std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:4318".to_string())
    }

    fn default_hook(violation: &Violation) {
        if !WARNED.swap(true, Ordering::Relaxed) {
            eprintln!(
                "mumei.monitor: no violation hook installed; reporting to stderr instead of {}",
                endpoint()
            );
        }
        eprintln!(
            "mumei.monitor.contract_violation atom={} boundary={} contract={} expression={} observed={}",
            violation.atom,
            violation.boundary,
            violation.contract,
            violation.expression,
            violation.observed.as_deref().unwrap_or("-")
        );
    }

    /// Record a violation. Never panics — the proof-aware monitor observes,
    /// it does not abort the program.
    pub fn record(violation: Violation) {
        if !enabled() {
            return;
        }
        let reported = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match HOOK.get() {
                Some(hook) => hook(&violation),
                None => default_hook(&violation),
            }
        }));
        if reported.is_err() {
            // A faulty host hook must not unwind through monitored code.
            eprintln!(
                "mumei.monitor.hook_panicked atom={} boundary={} contract={}",
                violation.atom, violation.boundary, violation.contract
            );
        }
    }

    /// Evaluate a contract without letting it unwind into the monitored call.
    ///
    /// A contract may divide by zero or overflow in a debug build, which would
    /// abort the very call the monitor only observes. Such an evaluation is
    /// reported as `observed = "evaluation panicked"` instead.
    pub fn check(violation: Violation, condition: impl FnOnce() -> bool) {
        if !enabled() {
            return;
        }
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(condition)) {
            Ok(true) => {}
            Ok(false) => record(violation),
            Err(_) => record(Violation {
                observed: Some("evaluation panicked".to_string()),
                ..violation
            }),
        }
    }
}
"#;

fn format_lowered_type_to_rust(lowered: &LoweredType) -> &'static str {
    match lowered {
        LoweredType::I64 => "i64",
        LoweredType::I32 => "i32",
        LoweredType::U64 => "u64",
        LoweredType::U32 => "u32",
        LoweredType::F64 => "f64",
        LoweredType::F32 => "f32",
        LoweredType::Bool => "i64",
        LoweredType::Str => "*const std::os::raw::c_char",
        LoweredType::Array(inner) if matches!(**inner, LoweredType::I64) => "*const i64",
        LoweredType::Array(_) | LoweredType::Other(_) => "i64",
    }
}

fn rust_type(type_name: &str, module_env: &ModuleEnv) -> String {
    let resolved = module_env.resolve_base_type(type_name);
    format_lowered_type_to_rust(&lower(&resolved)).to_string()
}

fn escape(contract: &str) -> String {
    contract.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Whether a clause's lowered `HirExpr` is a runtime-checkable monitor
/// condition: a subtree built only of `Number`, `Variable`, `BinaryOp` over
/// arithmetic/comparison/boolean `Op`s, the `IfThenElse` encoding of `!e`, or
/// `Call` (parity with the previous behaviour, which let `f(x)` through).
///
/// This replaces the old character whitelist on source text, which could not
/// see structure: it let `a => b` through (the characters pass, but the
/// interpolated text is not valid Rust) and accepted `a < b < c`, which the
/// parser normalizes into an `&&`-joined chain that raw text interpolation
/// rendered as invalid Rust. Rejected nodes — `Implies`, `Pow`, `StringLit`,
/// `Float`, `Match`, `Lambda`, `StructInit`, `FieldAccess`, `ArrayAccess`,
/// `VariantInit`, `ArrayLit`, `AtomRef`, `CallRef`, `Async`, `Await`,
/// `Perform`, `Task`, `TaskGroup`, `ChanSend`, `ChanRecv`, and any other
/// variant — route the clause through the `unchecked` path.
fn hir_expr_is_monitor_checkable(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Number(_) => true,
        // `::`-qualified names could resolve to *host* paths in generated Rust
        // (`std::process::exit`) — the monitor cannot vet them the way the
        // verifier vets wrapper contracts, so they stay `unchecked` (parity
        // with the old whitelist, which rejected `:`).
        HirExpr::Variable(name) => {
            !name.contains("::") && !matches!(name.as_str(), "forall" | "exists")
        }
        HirExpr::BinaryOp(l, op, r) => {
            matches!(
                op,
                HirBinOp::Add
                    | HirBinOp::Sub
                    | HirBinOp::Mul
                    | HirBinOp::Div
                    | HirBinOp::Eq
                    | HirBinOp::Neq
                    | HirBinOp::Gt
                    | HirBinOp::Lt
                    | HirBinOp::Ge
                    | HirBinOp::Le
                    | HirBinOp::And
                    | HirBinOp::Or
                    | HirBinOp::BitAnd
                    | HirBinOp::BitOr
                    | HirBinOp::BitXor
                    | HirBinOp::Shl
                    | HirBinOp::Shr
            ) && hir_expr_is_monitor_checkable(l)
                && hir_expr_is_monitor_checkable(r)
        }
        // `!e` lowers to `if e { false } else { true }` in the parser — accept
        // exactly that shape; any other if-expression is not a monitor
        // condition.
        HirExpr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            hir_stmt_bool_literal(then_branch) == Some(false)
                && hir_stmt_bool_literal(else_branch) == Some(true)
                && hir_expr_is_monitor_checkable(cond)
        }
        HirExpr::Call { name, args, .. } => {
            !name.contains("::")
                && !matches!(name.as_str(), "forall" | "exists")
                && args.iter().all(hir_expr_is_monitor_checkable)
        }
        _ => false,
    }
}

fn hir_stmt_bool_literal(stmt: &HirStmt) -> Option<bool> {
    if let HirStmt::Expr(HirExpr::Variable(v)) = stmt {
        match v.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    } else {
        None
    }
}

/// Checkability for one clause: the HIR node-kind check must pass *and* the
/// clause text must print into Rust. Both can fail independently (e.g. a
/// recovery placeholder `expr` parses but never prints).
fn clause_monitor_condition(
    clause: &mumei_core::hir::HirClause,
    vars: &ContractVars,
) -> Option<String> {
    let text = clause.text.trim();
    if text.is_empty() || text == "true" {
        return None;
    }
    let checkable = clause
        .expr
        .as_ref()
        .is_some_and(hir_expr_is_monitor_checkable);
    checkable.then(|| contract_text_to_host(text, vars, HostTarget::Rust))?
}

/// Generate the monitor module for a trust-boundary atom.
pub fn generate_monitor(emit_atom: &EmitAtom<'_>, module_env: &ModuleEnv) -> String {
    let signature = emit_atom.signature;
    let contract = emit_atom.contract;
    let meta = emit_atom.meta;
    let boundaries = &meta.trust_boundaries;
    let fn_name = signature.name.replace("::", "_");
    let params: Vec<(String, String)> = signature
        .params
        .iter()
        .map(|p| {
            let type_name = p.ty.as_deref().unwrap_or("i64");
            // `consume`/`ref` are mumei ownership markers — emit the bare name.
            (p.name.clone(), rust_type(type_name, module_env))
        })
        .collect();
    let return_type = rust_type(
        signature.return_type.as_deref().unwrap_or("i64"),
        module_env,
    );
    let boundary_tag = boundaries
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join("+");

    let mut rs = String::new();
    rs.push_str("// Auto-generated by mumei RuntimeMonitorEmitter (proof-aware observability).\n");
    rs.push_str("// Only trust boundaries are instrumented; fully proven atoms emit no code.\n");
    rs.push_str(MONITOR_RUNTIME);
    rs.push('\n');

    rs.push_str("extern \"C\" {\n");
    rs.push_str(&format!(
        "    fn {}({}) -> {};\n",
        fn_name,
        params
            .iter()
            .map(|(name, ty)| format!("{name}: {ty}"))
            .collect::<Vec<_>>()
            .join(", "),
        return_type
    ));
    rs.push_str("}\n\n");

    rs.push_str(&format!(
        "/// Monitored trust boundary `{}`.\n///\n",
        signature.name
    ));
    for kind in boundaries {
        rs.push_str(&format!("/// - {}: {}\n", kind.as_str(), kind.rationale()));
    }
    rs.push_str("///\n/// Contract violations are reported as OTel events, never panics.\n");

    rs.push_str(&format!(
        "pub fn {}_monitored({}) -> {} {{\n",
        fn_name,
        params
            .iter()
            .map(|(name, ty)| format!("{name}: {ty}"))
            .collect::<Vec<_>>()
            .join(", "),
        return_type
    ));

    let violation = |contract_kind: &str, contract: &str| {
        format!(
            "mumei_monitor::Violation {{\n            atom: \"{atom}\",\n            boundary: \"{boundary}\",\n            contract: \"{kind}\",\n            expression: \"{expr}\",\n            observed: None,\n        }}",
            atom = escape(&signature.name),
            boundary = escape(&boundary_tag),
            kind = contract_kind,
            expr = escape(contract),
        )
    };
    // Contract evaluation goes through `check`, so an arithmetic panic inside a
    // contract is reported rather than propagated into the monitored call.
    // `source_text` is the clause as written (kept for telemetry); `condition`
    // is the printed Rust expression that actually runs.
    let check = |contract_kind: &str, source_text: &str, condition: &str| {
        format!(
            "    mumei_monitor::check({}, || {});\n",
            violation(contract_kind, source_text),
            condition
        )
    };
    // An unsupported contract is left to verification, but the gap is reported
    // rather than only commented, so telemetry shows what is unchecked. The
    // expression stays a fixed literal: arbitrary source text (which failed
    // validation) is never embedded into generated code.
    let unchecked = |contract_kind: &str| {
        format!(
            "    // {contract_kind}: not expressible as a runtime condition, left to verification.\n    mumei_monitor::record({});\n",
            violation(
                &format!("{contract_kind}_unchecked"),
                "not a runtime-checkable expression"
            ),
        )
    };

    let vars = ContractVars::from_signature(signature, Some(module_env));

    // Emit one `check` (or `unchecked` record) per clause of the given kind.
    // Every requires/ensures clause is checked regardless of `mode`
    // (Plain/Assume/Check), per the design doc's Decisions; `Cover` clauses
    // are reachability queries and never become runtime assertions.
    let emit_clauses = |kind: HirClauseKind, label: &str| -> String {
        let mut out = String::new();
        for clause in &contract.clauses {
            if clause.kind != kind {
                continue;
            }
            let text = clause.text.trim();
            match clause_monitor_condition(clause, &vars) {
                Some(condition) => out.push_str(&check(label, text, &condition)),
                None if text.is_empty() || text == "true" => {}
                None => out.push_str(&unchecked(label)),
            }
        }
        out
    };

    // `effect_pre` is an assumption the proof makes about the caller's state.
    // It is only checkable when the host installs an effect-state probe; with
    // no probe the state is unobservable and nothing is reported.
    let mut effect_pre: Vec<(&String, &String)> = meta.effect_pre.iter().collect();
    effect_pre.sort();
    for (effect, state) in effect_pre {
        rs.push_str(&format!(
            "    if mumei_monitor::enabled() {{\n        if let Some(observed) = mumei_monitor::observed_effect_state(\"{effect}\") {{\n            if observed != \"{state}\" {{\n                mumei_monitor::record(mumei_monitor::Violation {{\n                    atom: \"{atom}\",\n                    boundary: \"{boundary}\",\n                    contract: \"effect_pre\",\n                    expression: \"{effect}: {state}\",\n                    observed: Some(observed),\n                }});\n            }}\n        }}\n    }}\n",
            effect = escape(effect),
            state = escape(state),
            atom = escape(&signature.name),
            boundary = escape(&boundary_tag),
        ));
    }

    rs.push_str(&emit_clauses(HirClauseKind::Requires, "requires"));

    // Quantified requires conjuncts get their own `check` lines: a hoisted
    // `forall(v, s, e, c)` becomes `(s..e).all(|v| c)`. Unlike clause payloads
    // (which keep the strict node check for parity), these are emitted from
    // the shared AST printer so bounds like `min(0, n)` and array indexing
    // `arr[i]` (`unsafe { *arr.add(i) }`) stay expressible; untranslatable
    // parts still degrade to `requires_unchecked`.
    for quantifier in &contract.quantifiers {
        let source = quantifier_source_text(quantifier);
        match quantifier_to_host(quantifier, &vars, HostTarget::Rust) {
            Some(condition) => rs.push_str(&check("requires", &source, &condition)),
            None => rs.push_str(&unchecked("requires")),
        }
    }

    rs.push_str(&format!(
        "    let result = unsafe {{ {}({}) }};\n",
        fn_name,
        params
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    rs.push_str(&emit_clauses(HirClauseKind::Ensures, "ensures"));

    rs.push_str("    result\n}\n");
    rs
}

/// Emitter that instruments trust boundaries only.
pub struct RuntimeMonitorEmitter;

impl Emitter for RuntimeMonitorEmitter {
    fn emit(
        &self,
        emit_atom: &EmitAtom<'_>,
        output_path: &Path,
        module_env: &ModuleEnv,
        _extern_blocks: &[ExternBlock],
    ) -> MumeiResult<Vec<Artifact>> {
        let boundaries = &emit_atom.meta.trust_boundaries;
        if boundaries.is_empty() {
            // Proven, self-contained atom: zero-cost, no artifact.
            return Ok(vec![]);
        }

        let source = generate_monitor(emit_atom, module_env);
        Ok(vec![Artifact {
            name: output_path.with_extension("monitor.rs"),
            data: source.into_bytes(),
            kind: ArtifactKind::Source,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mumei_core::hir::{lower_atom_metadata, HirAtom, HirEffectSet, HirExpr, HirStmt};
    use mumei_core::parser::ast::{Atom, Expr, Param, Span, Stmt, TrustLevel};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_atom(name: &str) -> Atom {
        Atom {
            name: name.to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![Param {
                name: "x".to_string(),
                type_name: Some("i64".to_string()),
                type_ref: None,
                is_ref: false,
                is_ref_mut: false,
                fn_contract_requires: None,
                fn_contract_ensures: None,
            }],
            trace_id: None,
            spec_metadata: HashMap::new(),
            clause_labels: Vec::new(),
            clause_modes: Vec::new(),
            covers: Vec::new(),
            requires: "x > 0".to_string(),
            forall_constraints: vec![],
            ensures: "result >= x".to_string(),
            body_expr: "x".to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: Some("i64".to_string()),
            decreases: None,
            span: Span::default(),
            effect_pre: HashMap::new(),
            effect_post: HashMap::new(),
        }
    }

    fn hir(atom: Atom) -> HirAtom {
        let body = HirStmt::Expr(HirExpr::Number(0));
        let (signature, contract, meta) = lower_atom_metadata(&atom, &body, None);
        HirAtom {
            body,
            signature,
            contract,
            meta,
            atom,
            body_stmt: Stmt::Expr(Expr::Number(0), Span::default()),
            effect_set: HirEffectSet::default(),
        }
    }

    fn emit(atom: Atom) -> Vec<Artifact> {
        RuntimeMonitorEmitter
            .emit(
                &hir(atom).emit_view(),
                &PathBuf::from("out/atom"),
                &ModuleEnv::new(),
                &[],
            )
            .expect("emit succeeds")
    }

    #[test]
    fn proven_pure_atom_emits_no_monitor() {
        assert!(emit(make_atom("pure_add")).is_empty());
    }

    #[test]
    fn trusted_atom_emits_a_monitor() {
        let mut atom = make_atom("read_clock");
        atom.trust_level = TrustLevel::Trusted;
        let artifacts = emit(atom);
        assert_eq!(artifacts.len(), 1);
        let source = String::from_utf8(artifacts[0].data.clone()).expect("utf8");
        assert!(source.contains("pub fn read_clock_monitored(x: i64) -> i64"));
        assert!(source.contains("boundary: \"trusted_atom\""));
        assert!(source.contains("contract: \"requires\""));
        assert!(source.contains("contract: \"ensures\""));
        assert!(
            !source.contains("panic!") && !source.contains("assert!"),
            "monitors report, they do not abort: {source}"
        );
        assert!(source.contains("OTEL_ENABLED"));
        assert!(source.contains("OTEL_EXPORTER_OTLP_ENDPOINT"));
    }

    #[test]
    fn contracts_outside_the_expression_subset_are_not_lowered() {
        let mut atom = make_atom("read_clock");
        atom.trust_level = TrustLevel::Trusted;
        atom.requires = "x > 0) { std::process::exit(1); } if (true".to_string();
        atom.ensures = "forall i: i64. i > 0".to_string();
        let artifacts = emit(atom);
        let source = String::from_utf8(artifacts[0].data.clone()).expect("utf8");
        assert!(!source.contains("std::process::exit"));
        assert!(!source.contains("forall"));
        assert!(!source.contains("contract: \"requires\""));
        assert!(!source.contains("contract: \"ensures\""));
        assert!(source.contains("not expressible as a runtime condition"));
    }

    #[test]
    fn generated_runtime_contains_a_panic_boundary_for_host_hooks() {
        let mut atom = make_atom("read_clock");
        atom.trust_level = TrustLevel::Trusted;
        let artifacts = emit(atom);
        let source = String::from_utf8(artifacts[0].data.clone()).expect("utf8");
        assert!(source.contains("catch_unwind"));
        assert!(source.contains("mumei.monitor.hook_panicked"));
    }

    #[test]
    fn effect_pre_atom_is_instrumented_with_its_boundary_tag() {
        let mut atom = make_atom("send_request");
        atom.effect_pre
            .insert("OrderChannel".to_string(), "Idle".to_string());
        let artifacts = emit(atom);
        let source = String::from_utf8(artifacts[0].data.clone()).expect("utf8");
        assert!(source.contains("boundary: \"effect_pre_override\""));
        assert!(source.contains("mumei_monitor::observed_effect_state(\"OrderChannel\")"));
        assert!(source.contains("if observed != \"Idle\""));
        assert!(source.contains("contract: \"effect_pre\""));
    }

    #[test]
    fn monitor_is_a_no_op_when_otel_is_disabled() {
        let mut atom = make_atom("read_clock");
        atom.trust_level = TrustLevel::Trusted;
        let artifacts = emit(atom);
        let source = String::from_utf8(artifacts[0].data.clone()).expect("utf8");
        // Contracts are evaluated inside `check`, which returns before touching
        // the condition unless OTEL_ENABLED is truthy.
        assert_eq!(source.matches("mumei_monitor::check(").count(), 2);
        assert!(source.contains("pub fn check(violation: Violation, condition: impl FnOnce() -> bool) {\n        if !enabled() {\n            return;"));
    }
}
