//! Replayability verification for non-deterministic effects.
//!
//! `Random`, `Clock` and `ExternalInput` (see `std/effects.mm`) mark atoms whose
//! behaviour depends on a source outside the program. Mumei isolates those
//! sources at the type level: an atom that declares such an effect must
//! receive every non-deterministic value through an explicit *witness*
//! parameter (`seed`, `timestamp`, `input`, ...) and must thread that witness
//! into each `perform` of the effect. Once the witness is an ordinary
//! parameter, verification treats it like any other universally-quantified
//! input, and re-running the atom with the same arguments replays the same
//! trace (the perform result is modelled as an uninterpreted function of its
//! arguments, see `nondeterministic_perform_result`).
//!
//! Pure atoms (no `effects:` clause) never reach this check: performing or
//! calling a non-deterministic source from a pure atom is already rejected by
//! effect containment.

use super::super::module_env::ModuleEnv;
use super::super::nlae_reporter::FAILURE_EFFECT_NOT_ALLOWED;
use super::super::types::{MumeiError, MumeiResult};
use crate::parser::{Atom, Expr, Stmt};
use serde_json::json;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::Path;

/// Built-in non-deterministic effects and the parameter names accepted as the
/// explicit witness for each. A parameter matches when its name equals a
/// witness name or starts with `<witness>_` (e.g. `seed_a`, `timestamp_ms`).
const NONDETERMINISTIC_EFFECTS: &[(&str, &[&str])] = &[
    ("Random", &["seed"]),
    ("Clock", &["timestamp", "now", "clock"]),
    ("ExternalInput", &["input", "external_input"]),
];

/// Witness parameter names for `effect_name`, or `None` if the effect is not a
/// built-in non-deterministic source.
pub(crate) fn nondeterministic_witnesses(effect_name: &str) -> Option<&'static [&'static str]> {
    NONDETERMINISTIC_EFFECTS
        .iter()
        .find(|(name, _)| *name == effect_name)
        .map(|(_, witnesses)| *witnesses)
}

/// Resolve `effect_name` (possibly a composite or a child via `parent:`) to the
/// built-in non-deterministic leaf it stands for, if any.
pub(crate) fn nondeterministic_root(
    module_env: &ModuleEnv,
    effect_name: &str,
) -> Option<&'static str> {
    NONDETERMINISTIC_EFFECTS
        .iter()
        .map(|(name, _)| *name)
        .find(|root| effect_name == *root || module_env.is_subeffect(effect_name, root))
}

fn param_is_witness(param_name: &str, witnesses: &[&str]) -> bool {
    witnesses.iter().any(|w| {
        param_name == *w
            || param_name
                .strip_prefix(w)
                .is_some_and(|rest| rest.starts_with('_'))
    })
}

/// Non-deterministic leaf effects declared (positively) by `atom`, in a
/// deterministic order.
pub(crate) fn declared_nondeterministic_effects(
    atom: &Atom,
    module_env: &ModuleEnv,
) -> Vec<&'static str> {
    let positive: Vec<crate::parser::Effect> = atom
        .effects
        .iter()
        .filter(|e| !e.negated)
        .cloned()
        .collect();
    let leaves = module_env.resolve_leaf_effects_from_effects(&positive);
    let mut roots = BTreeSet::new();
    for leaf in &leaves {
        if let Some(root) = nondeterministic_root(module_env, leaf) {
            roots.insert(root);
        }
    }
    roots.into_iter().collect()
}

/// Names of `atom`'s parameters that act as witnesses for `effect`.
pub(crate) fn witness_params<'a>(atom: &'a Atom, effect: &str) -> Vec<&'a str> {
    let Some(witnesses) = nondeterministic_witnesses(effect) else {
        return Vec::new();
    };
    atom.params
        .iter()
        .filter(|p| param_is_witness(&p.name, witnesses))
        .map(|p| p.name.as_str())
        .collect()
}

/// A `perform <effect>.<operation>(args)` site together with the witness
/// parameters its arguments are (transitively) derived from.
struct PerformSite<'a> {
    effect: &'a str,
    operation: &'a str,
    witnesses: BTreeSet<&'a str>,
}

/// Walks the body in program order, tracking which locals are derived from a
/// witness parameter (`let s2 = seed + 1;` makes `s2` a carrier of `seed`).
struct PerformCollector<'a> {
    /// local / parameter name -> witness parameters it carries
    derived: HashMap<&'a str, BTreeSet<&'a str>>,
    sites: Vec<PerformSite<'a>>,
}

impl<'a> PerformCollector<'a> {
    fn new(witnesses: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            derived: witnesses
                .into_iter()
                .map(|w| (w, BTreeSet::from([w])))
                .collect(),
            sites: Vec::new(),
        }
    }

    /// Witness parameters that `expr`'s value is guaranteed to be derived from.
    /// Data-flow through operators, calls, aggregates and performs is a union
    /// (any witness-carrying operand taints the result); a value chosen by an
    /// `if`/`match` is derived only from witnesses common to *every* branch,
    /// and the condition / scrutinee contributes nothing.
    fn carried_by(&self, expr: &'a Expr) -> BTreeSet<&'a str> {
        match expr {
            Expr::Variable(name) => self.derived.get(name.as_str()).cloned().unwrap_or_default(),
            Expr::ArrayAccess(name, idx) => {
                let mut out = self.derived.get(name.as_str()).cloned().unwrap_or_default();
                out.extend(self.carried_by(idx));
                out
            }
            Expr::BinaryOp(l, _, r) => {
                let mut out = self.carried_by(l);
                out.extend(self.carried_by(r));
                out
            }
            Expr::FieldAccess(e, _) | Expr::Await { expr: e } | Expr::ChanRecv { channel: e } => {
                self.carried_by(e)
            }
            Expr::Call(_, args) | Expr::Perform { args, .. } | Expr::ArrayLit(args) => {
                args.iter().flat_map(|a| self.carried_by(a)).collect()
            }
            Expr::CallRef { callee, args } => {
                let mut out = self.carried_by(callee);
                for a in args {
                    out.extend(self.carried_by(a));
                }
                out
            }
            Expr::StructInit { fields, .. } => fields
                .iter()
                .flat_map(|(_, v)| self.carried_by(v))
                .collect(),
            Expr::ChanSend { channel, value } => {
                let mut out = self.carried_by(channel);
                out.extend(self.carried_by(value));
                out
            }
            Expr::IfThenElse {
                then_branch,
                else_branch,
                ..
            } => {
                let then = self.value_of_block(then_branch);
                let els = self.value_of_block(else_branch);
                then.intersection(&els).copied().collect()
            }
            Expr::Match { target, arms } => {
                let scrutinee = self.carried_by(target);
                let mut acc: Option<BTreeSet<&'a str>> = None;
                for arm in arms {
                    let mut scratch = self.scratch();
                    let mut bound = Vec::new();
                    pattern_vars(&arm.pattern, &mut bound);
                    for name in bound {
                        scratch.bind_set(name, scrutinee.clone());
                    }
                    let value = scratch.value_of_block(&arm.body);
                    acc = Some(match acc {
                        None => value,
                        Some(acc) => acc.intersection(&value).copied().collect(),
                    });
                }
                acc.unwrap_or_default()
            }
            Expr::Number(_)
            | Expr::Float(_)
            | Expr::StringLit(_)
            | Expr::AtomRef { .. }
            | Expr::Async { .. }
            | Expr::Lambda { .. } => BTreeSet::new(),
        }
    }

    fn scratch(&self) -> Self {
        Self {
            derived: self.derived.clone(),
            sites: Vec::new(),
        }
    }

    /// Provenance of the value a block yields (its trailing expression), after
    /// applying the block's own bindings on a scratch copy of the state.
    fn value_of_block(&self, block: &'a Stmt) -> BTreeSet<&'a str> {
        match block {
            Stmt::Block(stmts, _) => {
                let Some((last, init)) = stmts.split_last() else {
                    return BTreeSet::new();
                };
                let mut scratch = self.scratch();
                for s in init {
                    scratch.stmt(s);
                }
                scratch.value_of_block(last)
            }
            Stmt::Expr(e, _) => self.carried_by(e),
            _ => BTreeSet::new(),
        }
    }

    fn bind(&mut self, var: &'a str, value: &'a Expr) {
        let carried = self.carried_by(value);
        self.bind_set(var, carried);
    }

    fn bind_set(&mut self, var: &'a str, carried: BTreeSet<&'a str>) {
        if carried.is_empty() {
            self.derived.remove(var);
        } else {
            self.derived.insert(var, carried);
        }
    }

    /// Run `f` on each alternative path starting from the current state and
    /// continue with the meet of the resulting states: a local is a witness
    /// carrier after the join only if it is one on every path.
    fn alternatives(&mut self, paths: &[&'a Stmt], mut f: impl FnMut(&mut Self, &'a Stmt)) {
        let entry = self.derived.clone();
        let mut joined: Option<HashMap<&'a str, BTreeSet<&'a str>>> = None;
        for path in paths {
            self.derived = entry.clone();
            f(self, path);
            joined = Some(match joined {
                None => std::mem::take(&mut self.derived),
                Some(acc) => meet(acc, std::mem::take(&mut self.derived)),
            });
        }
        self.derived = joined.unwrap_or(entry);
    }

    /// A loop body may run zero or more times: iterate to a fixpoint so that a
    /// binding lost on any iteration is not counted as a witness on the next.
    fn loop_body(&mut self, cond: &'a Expr, body: &'a Stmt) {
        loop {
            let entry = self.derived.clone();
            let sites_len = self.sites.len();
            self.expr(cond);
            self.stmt(body);
            let next = meet(entry.clone(), std::mem::take(&mut self.derived));
            if next == entry {
                self.derived = next;
                return;
            }
            self.sites.truncate(sites_len);
            self.derived = next;
        }
    }

    /// Analyse a body that may run zero times, later, or concurrently (lambda,
    /// async block, task): assignments inside it that drop a witness must be
    /// honoured, but bindings it establishes may never have happened, so the
    /// state afterwards is the meet of before and after. `shadowed` names are
    /// local to the body and keep their outer provenance.
    fn deferred_body(&mut self, shadowed: &[&'a str], body: &'a Stmt) {
        let outer = self.derived.clone();
        for name in shadowed {
            self.derived.remove(name);
        }
        self.stmt(body);
        let mut after = std::mem::take(&mut self.derived);
        for name in shadowed {
            match outer.get(name) {
                Some(carried) => {
                    after.insert(name, carried.clone());
                }
                None => {
                    after.remove(name);
                }
            }
        }
        self.derived = meet(outer, after);
    }

    fn match_arm(&mut self, scrutinee: BTreeSet<&'a str>, arm: &'a crate::parser::MatchArm) {
        let mut bound = Vec::new();
        pattern_vars(&arm.pattern, &mut bound);
        for name in bound {
            self.bind_set(name, scrutinee.clone());
        }
        if let Some(guard) = &arm.guard {
            self.expr(guard);
        }
        self.stmt(&arm.body);
    }

    fn expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Perform {
                effect,
                operation,
                args,
            } => {
                let witnesses = args.iter().flat_map(|arg| self.carried_by(arg)).collect();
                self.sites.push(PerformSite {
                    effect,
                    operation,
                    witnesses,
                });
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr::Call(_, args) | Expr::ArrayLit(args) => {
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr::CallRef { callee, args } => {
                self.expr(callee);
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr::StructInit { fields, .. } => {
                for (_, value) in fields {
                    self.expr(value);
                }
            }
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(cond);
                self.alternatives(&[then_branch, else_branch], |c, branch| c.stmt(branch));
            }
            Expr::BinaryOp(l, _, r) => {
                self.expr(l);
                self.expr(r);
            }
            Expr::ArrayAccess(_, idx) => self.expr(idx),
            Expr::FieldAccess(e, _) | Expr::Await { expr: e } => self.expr(e),
            Expr::Async { body } => self.deferred_body(&[], body),
            Expr::Lambda { params, body, .. } => {
                let shadowed: Vec<&'a str> = params.iter().map(|p| p.name.as_str()).collect();
                self.deferred_body(&shadowed, body);
            }
            Expr::Match { target, arms } => {
                self.expr(target);
                let scrutinee = self.carried_by(target);
                let entry = self.derived.clone();
                let mut joined: Option<HashMap<&'a str, BTreeSet<&'a str>>> = None;
                for arm in arms {
                    self.derived = entry.clone();
                    self.match_arm(scrutinee.clone(), arm);
                    joined = Some(match joined {
                        None => std::mem::take(&mut self.derived),
                        Some(acc) => meet(acc, std::mem::take(&mut self.derived)),
                    });
                }
                self.derived = joined.unwrap_or(entry);
            }
            Expr::ChanSend { channel, value } => {
                self.expr(channel);
                self.expr(value);
            }
            Expr::ChanRecv { channel } => self.expr(channel),
            Expr::Number(_)
            | Expr::Float(_)
            | Expr::StringLit(_)
            | Expr::Variable(_)
            | Expr::AtomRef { .. } => {}
        }
    }

    fn stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Block(stmts, _) => {
                for s in stmts {
                    self.stmt(s);
                }
            }
            Stmt::Let { var, value, .. } | Stmt::Assign { var, value, .. } => {
                self.expr(value);
                self.bind(var, value);
            }
            Stmt::ArrayStore {
                array,
                index,
                value,
                ..
            } => {
                self.expr(index);
                self.expr(value);
                // An element is overwritten: the array carries only witnesses
                // common to its previous contents and the stored value.
                let stored = self.carried_by(value);
                let carried: BTreeSet<&'a str> = self
                    .derived
                    .get(array.as_str())
                    .map(|prev| prev.intersection(&stored).copied().collect())
                    .unwrap_or_default();
                self.bind_set(array, carried);
            }
            Stmt::While { cond, body, .. } => self.loop_body(cond, body),
            Stmt::Acquire { body, .. } => self.stmt(body),
            Stmt::Task { body, .. } => self.deferred_body(&[], body),
            Stmt::TaskGroup { children, .. } => {
                for child in children {
                    self.stmt(child);
                }
            }
            Stmt::Expr(e, _) => self.expr(e),
            Stmt::Cancel { .. } => {}
        }
    }
}

/// Pointwise intersection of two provenance maps.
fn meet<'a>(
    mut a: HashMap<&'a str, BTreeSet<&'a str>>,
    b: HashMap<&'a str, BTreeSet<&'a str>>,
) -> HashMap<&'a str, BTreeSet<&'a str>> {
    a.retain(|name, carried| match b.get(name) {
        Some(other) => {
            carried.retain(|w| other.contains(w));
            !carried.is_empty()
        }
        None => false,
    });
    a
}

fn pattern_vars<'a>(pattern: &'a crate::parser::Pattern, out: &mut Vec<&'a str>) {
    match pattern {
        crate::parser::Pattern::Variable(name) => out.push(name),
        crate::parser::Pattern::Variant { fields, .. } => {
            for field in fields {
                pattern_vars(field, out);
            }
        }
        crate::parser::Pattern::Wildcard | crate::parser::Pattern::Literal(_) => {}
    }
}

/// Structured description of a replayability violation, saved to `report.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayViolation {
    pub atom: String,
    pub effect: &'static str,
    pub accepted_witnesses: Vec<&'static str>,
    /// `None` when the signature lacks a witness parameter; `Some(op)` when the
    /// `perform <effect>.<op>` call site does not thread the witness through.
    pub perform_operation: Option<String>,
}

impl ReplayViolation {
    fn suggested_signature(&self) -> String {
        format!("{}: i64", self.accepted_witnesses[0])
    }
}

/// Verify that every non-deterministic effect declared by `atom` is fed through
/// an explicit witness parameter (signature) and that each `perform` of the
/// effect receives a value derived from that witness (body).
pub(crate) fn check_replayability(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
) -> Result<(), ReplayViolation> {
    let roots = declared_nondeterministic_effects(atom, module_env);
    if roots.is_empty() {
        return Ok(());
    }

    let mut collector =
        PerformCollector::new(roots.iter().flat_map(|root| witness_params(atom, root)));
    collector.stmt(body_stmt);

    for root in roots {
        let accepted = nondeterministic_witnesses(root).unwrap_or(&[]).to_vec();
        let witnesses = witness_params(atom, root);
        if witnesses.is_empty() {
            return Err(ReplayViolation {
                atom: atom.name.clone(),
                effect: root,
                accepted_witnesses: accepted,
                perform_operation: None,
            });
        }
        for site in &collector.sites {
            if nondeterministic_root(module_env, site.effect) != Some(root) {
                continue;
            }
            if !witnesses.iter().any(|w| site.witnesses.contains(w)) {
                return Err(ReplayViolation {
                    atom: atom.name.clone(),
                    effect: root,
                    accepted_witnesses: accepted,
                    perform_operation: Some(site.operation.to_string()),
                });
            }
        }
    }
    Ok(())
}

/// Run `check_replayability`, writing `report.json` and returning a
/// `MumeiError` on violation.
pub(crate) fn verify_replayability(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
    output_dir: &Path,
) -> MumeiResult<()> {
    check_replayability(atom, body_stmt, module_env).map_err(|violation| {
        save_replayability_report(output_dir, &violation);
        let accepted = violation.accepted_witnesses.join(" | ");
        match &violation.perform_operation {
            None => MumeiError::verification_at(
                format!(
                    "Replayability violation: atom '{}' declares non-deterministic effect '{}' \
                     but has no explicit witness parameter ({}). Non-deterministic sources must \
                     be received as parameters so the same inputs replay the same output.",
                    violation.atom, violation.effect, accepted
                ),
                atom.span.clone(),
            )
            .with_help(format!(
                "Add a parameter `{}` to atom '{}' and pass it to every `perform {}.<op>(...)`.",
                violation.suggested_signature(),
                violation.atom,
                violation.effect
            )),
            Some(op) => MumeiError::verification_at(
                format!(
                    "Replayability violation: atom '{}' performs '{}.{}' without threading its \
                     witness parameter ({}) through the call. The non-deterministic source must \
                     be derived from the explicit parameter.",
                    violation.atom, violation.effect, op, accepted
                ),
                atom.span.clone(),
            )
            .with_help(format!(
                "Pass the witness parameter as an argument: `perform {}.{}({})`.",
                violation.effect,
                op,
                witness_params(atom, violation.effect)
                    .first()
                    .copied()
                    .unwrap_or(violation.accepted_witnesses[0])
            )),
        }
    })
}

/// Save a replayability violation report to `report.json` for self-healing integration.
fn save_replayability_report(output_dir: &Path, violation: &ReplayViolation) {
    let witness = violation.suggested_signature();
    let (reason, fix) = match &violation.perform_operation {
        None => (
            format!(
                "Replayability violation: atom '{}' declares non-deterministic effect '{}' \
                 without a witness parameter",
                violation.atom, violation.effect
            ),
            format!("Add parameter `{}` to atom '{}'", witness, violation.atom),
        ),
        Some(op) => (
            format!(
                "Replayability violation: atom '{}' performs '{}.{}' without its witness parameter",
                violation.atom, violation.effect, op
            ),
            format!(
                "Pass the witness parameter to `perform {}.{}`",
                violation.effect, op
            ),
        ),
    };
    let mut report = json!({
        "status": "failed",
        "atom": violation.atom,
        "failure_type": FAILURE_EFFECT_NOT_ALLOWED,
        "violation_type": "replayability",
        "effect_violation": {
            "atom": violation.atom,
            "effect": violation.effect,
            "accepted_witnesses": violation.accepted_witnesses,
            "perform_operation": violation.perform_operation,
            "suggested_fixes": [fix],
            "resolution_paths": [
                {
                    "strategy": "witness_parameter",
                    "description": format!(
                        "Receive the '{}' source through an explicit parameter and thread it into every perform",
                        violation.effect
                    ),
                    "fix_type": "signature_change",
                    "target": violation.atom,
                    "change": witness,
                },
                {
                    "strategy": "isolation",
                    "description": format!("Remove '{}' from the effects declaration and drop the perform", violation.effect),
                    "fix_type": "body_change",
                    "target": violation.atom,
                }
            ]
        },
        "reason": reason,
    });
    report["structured_feedback"] = json!(
        crate::structured_feedback::StructuredFeedback::from_report(&report)
    );
    let _ = fs::create_dir_all(output_dir);
    let _ = fs::write(
        output_dir.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap_or_else(|_| report.to_string()),
    );
}
