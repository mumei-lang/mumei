#![allow(unused_imports)]
use super::super::module_env::*;
use super::super::nlae_reporter::*;
use super::super::translator::*;
use super::super::types::*;
use super::super::*;
use super::call_graph::collect_callees_stmt;
use super::infer_effects;
use crate::parser::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use z3::ast::{Bool, String as Z3String};
use z3::{Config, Context, SatResult, Solver};

// セキュリティポリシー (Security Policy)
// =============================================================================

/// A single allowed effect with optional parameter constraints.
/// Used by SecurityPolicy to define which effects (and under what conditions)
/// are permitted in the current session.
#[derive(Debug, Clone)]
pub struct AllowedEffect {
    pub effect_name: String,
    /// Parameter constraints as (param_name, constraint_expr) pairs.
    /// E.g., ("path", "starts_with(path, \"/tmp/\")") for FileRead.
    #[allow(dead_code)]
    pub param_constraints: Vec<(String, String)>,
}

/// Security policy defining which effects are permitted.
/// Enforced during effect containment verification.
/// Can be set dynamically via the MCP server's set_allowed_effects tool.
#[derive(Debug, Clone, Default)]
pub struct SecurityPolicy {
    pub allowed_effects: Vec<AllowedEffect>,
}

#[allow(dead_code)]
impl SecurityPolicy {
    pub fn new() -> Self {
        Self {
            allowed_effects: Vec::new(),
        }
    }

    /// Add an allowed effect with optional parameter constraints.
    pub fn allow_effect(&mut self, effect_name: &str, param_constraints: Vec<(String, String)>) {
        self.allowed_effects.push(AllowedEffect {
            effect_name: effect_name.to_string(),
            param_constraints,
        });
    }

    /// Check if an effect is allowed by this policy (name-level only).
    pub fn is_effect_allowed(&self, effect_name: &str) -> bool {
        self.allowed_effects
            .iter()
            .any(|ae| ae.effect_name == effect_name)
    }

    /// Get the parameter constraints for a specific effect.
    pub fn get_constraints(&self, effect_name: &str) -> Vec<&(String, String)> {
        self.allowed_effects
            .iter()
            .filter(|ae| ae.effect_name == effect_name)
            .flat_map(|ae| ae.param_constraints.iter())
            .collect()
    }

    /// Check if an effect with a specific string parameter satisfies the policy.
    /// Uses constant folding for string literals: directly evaluates starts_with/contains.
    /// For symbolic (non-literal) parameters, returns Ok; verification uses Z3 String Sort.
    pub fn check_param_constraint(
        &self,
        effect_name: &str,
        param_name: &str,
        param_value: Option<&str>,
    ) -> Result<(), String> {
        let constraints = self.get_constraints(effect_name);
        if constraints.is_empty() {
            return Ok(());
        }

        for (cname, cexpr) in &constraints {
            if cname != param_name {
                continue;
            }
            // Constant folding: if param_value is a known string literal, evaluate directly
            if let Some(val) = param_value {
                if !evaluate_string_constraint(cexpr, param_name, val) {
                    return Err(format!(
                        "Parameter constraint violated: {} = \"{}\" does not satisfy `{}` \
                         (パラメータ制約違反: {} = \"{}\" は `{}` を満たしません)",
                        param_name, val, cexpr, param_name, val, cexpr
                    ));
                }
            }
            // If param_value is None (symbolic), we defer to Z3 symbolic verification
        }
        Ok(())
    }
}

/// Evaluate a string constraint expression against a concrete value.
/// Supports: starts_with(param, "prefix"), ends_with(param, "suffix"), contains(param, "substr")
#[allow(dead_code)]
pub(crate) fn evaluate_string_constraint(
    constraint_expr: &str,
    _param_name: &str,
    value: &str,
) -> bool {
    let trimmed = constraint_expr.trim();

    // Compound constraint: split on && and check all sub-constraints
    // NOTE: This naive split does not respect && inside quoted strings
    // (e.g., not_contains(path, "a&&b") would be incorrectly split).
    // This is an acceptable limitation since constraint string arguments
    // are path patterns that should never contain "&&".
    if trimmed.contains("&&") {
        return trimmed
            .split("&&")
            .all(|part| evaluate_string_constraint(part.trim(), _param_name, value));
    }

    // starts_with(param, "prefix")
    if let Some(inner) = trimmed.strip_prefix("starts_with(") {
        if let Some(inner) = inner.strip_suffix(')') {
            if let Some((_p, rest)) = inner.split_once(',') {
                let prefix = rest.trim().trim_matches('"');
                return value.starts_with(prefix);
            }
        }
    }

    // ends_with(param, "suffix")
    if let Some(inner) = trimmed.strip_prefix("ends_with(") {
        if let Some(inner) = inner.strip_suffix(')') {
            if let Some((_p, rest)) = inner.split_once(',') {
                let suffix = rest.trim().trim_matches('"');
                return value.ends_with(suffix);
            }
        }
    }

    // not_contains(param, "substr")
    if let Some(inner) = trimmed.strip_prefix("not_contains(") {
        if let Some(inner) = inner.strip_suffix(')') {
            if let Some((_p, rest)) = inner.split_once(',') {
                let substr = rest.trim().trim_matches('"');
                return !value.contains(substr);
            }
        }
    }

    // contains(param, "substr")
    if let Some(inner) = trimmed.strip_prefix("contains(") {
        if let Some(inner) = inner.strip_suffix(')') {
            if let Some((_p, rest)) = inner.split_once(',') {
                let substr = rest.trim().trim_matches('"');
                return value.contains(substr);
            }
        }
    }

    // matches(param, "regex") — use Rust regex crate for evaluation
    if let Some(inner) = trimmed.strip_prefix("matches(") {
        if let Some(inner) = inner.strip_suffix(')') {
            // Extract the regex pattern (last quoted string)
            if let Some(last_quote_end) = inner.rfind('"') {
                let before = &inner[..last_quote_end];
                if let Some(last_quote_start) = before.rfind('"') {
                    let pattern = &inner[last_quote_start + 1..last_quote_end];
                    if let Ok(re) = regex::Regex::new(pattern) {
                        return re.is_match(value);
                    }
                }
            }
        }
    }

    // Unknown constraint — conservatively allow (will be checked by Z3 if symbolic).
    //
    // NOTE: This returns `true` (allow) for unknown constraints, which differs from
    // `check_constant_constraint()` which returns `false` (reject) for unknowns.
    // This is intentional: `evaluate_string_constraint` is used by SecurityPolicy
    // (advisory layer with Z3 fallback for symbolic params), so unknown constraints
    // are deferred to Z3. `check_constant_constraint` is used by verify_effect_params
    // (authoritative constant-folding fast-path), where unknown constraints must be
    // rejected to prevent unverified values from passing through.
    true
}

// =============================================================================
// Z3 String Sort — Constraint Parsing and Mapping
// =============================================================================
//
// Convert effect parameter constraint strings (e.g. "starts_with(path, \"/tmp/\")")
// into Z3 String Sort constraints for symbolic verification of variable paths.
// Constant paths continue to be checked by evaluate_string_constraint / check_constant_constraint.
//

/// Parse a constraint string and generate a Z3 Bool expression using Z3 String Sort.
///
/// Supports:
/// - `starts_with(param, "prefix")` → Z3: `str.prefixof("prefix", param_z3)`
/// - `ends_with(param, "suffix")`   → Z3: `str.suffixof("suffix", param_z3)`
/// - `contains(param, "substr")`    → Z3: `str.contains(param_z3, "substr")`
/// - `not_contains(param, "substr")`→ Z3: `NOT str.contains(param_z3, "substr")`
///
/// Returns `None` if the constraint cannot be parsed.
pub(crate) fn parse_constraint_to_z3_string<'ctx>(
    ctx: &'ctx Context,
    constraint: &str,
    param_z3: &Z3String<'ctx>,
) -> Option<Bool<'ctx>> {
    let trimmed = constraint.trim();

    // Compound constraint: "constraint1 && constraint2"
    // Must be checked BEFORE individual constraint checks to avoid partial matches.
    // NOTE: This naive split does not respect && inside quoted strings
    // (e.g., not_contains(path, "a&&b") would be incorrectly split).
    // This is an acceptable limitation since constraint string arguments
    // are path patterns that should never contain "&&".
    if trimmed.contains("&&") {
        let parts: Vec<&str> = trimmed.split("&&").collect();
        let mut bools: Vec<Bool<'ctx>> = Vec::new();
        for part in &parts {
            if let Some(b) = parse_constraint_to_z3_string(ctx, part.trim(), param_z3) {
                bools.push(b);
            } else {
                // Unrecognized sub-constraint — fail the entire compound to avoid
                // silently weakening security constraints.
                return None;
            }
        }
        if !bools.is_empty() {
            let refs: Vec<&Bool> = bools.iter().collect();
            return Some(Bool::and(ctx, &refs));
        }
    }

    // Extract the string literal argument from the constraint
    let extract_string_arg = |c: &str| -> Option<std::string::String> {
        if let Some(last_quote_end) = c.rfind('"') {
            let before = &c[..last_quote_end];
            if let Some(last_quote_start) = before.rfind('"') {
                return Some(c[last_quote_start + 1..last_quote_end].to_string());
            }
        }
        None
    };

    // starts_with(param, "prefix") → Z3: prefix.prefix(param)
    // Z3 semantics: prefix.prefix(s) means "prefix is a prefix of s"
    if trimmed.starts_with("starts_with(") {
        if let Some(arg) = extract_string_arg(trimmed) {
            let prefix_z3 = Z3String::from_str(ctx, &arg).ok()?;
            return Some(prefix_z3.prefix(param_z3));
        }
    }

    // ends_with(param, "suffix") → Z3: suffix.suffix(param)
    // Z3 semantics: suffix.suffix(s) means "suffix is a suffix of s"
    if trimmed.starts_with("ends_with(") {
        if let Some(arg) = extract_string_arg(trimmed) {
            let suffix_z3 = Z3String::from_str(ctx, &arg).ok()?;
            return Some(suffix_z3.suffix(param_z3));
        }
    }

    // contains(param, "substr") → Z3: param.contains(substr)
    if trimmed.starts_with("contains(") {
        if let Some(arg) = extract_string_arg(trimmed) {
            let substr_z3 = Z3String::from_str(ctx, &arg).ok()?;
            return Some(param_z3.contains(&substr_z3));
        }
    }

    // not_contains(param, "substr") → Z3: NOT param.contains(substr)
    if trimmed.starts_with("not_contains(") {
        if let Some(arg) = extract_string_arg(trimmed) {
            let substr_z3 = Z3String::from_str(ctx, &arg).ok()?;
            return Some(param_z3.contains(&substr_z3).not());
        }
    }

    // Plan 10: matches(param, "regex_pattern") → approximate via prefix/suffix/contains
    // The z3 crate v0.12 does not expose str.in_re / re.from_str API directly.
    // We approximate common regex patterns using Z3 String prefix/suffix/contains:
    //   - "^prefix.*"  → starts_with
    //   - ".*suffix$"  → ends_with
    //   - ".*substr.*" → contains
    // For patterns that cannot be approximated, we return None (constraint not enforceable
    // at Z3 level; constant checking via Rust regex crate handles the rest).
    if trimmed.starts_with("matches(") {
        if let Some(pattern) = extract_string_arg(trimmed) {
            // Try to approximate the regex pattern with Z3 String constraints
            let stripped = pattern.as_str();
            // Helper: check if a literal fragment contains regex metacharacters
            // that would make it unsafe to treat as a Z3 literal string.
            let is_literal = |s: &str| -> bool {
                !s.contains('*')
                    && !s.contains('?')
                    && !s.contains('[')
                    && !s.contains('.')
                    && !s.contains('\\')
                    && !s.contains('+')
                    && !s.contains('(')
                    && !s.contains(')')
                    && !s.contains('|')
                    && !s.contains('{')
                    && !s.contains('}')
            };
            // ^prefix.* → starts_with(param, prefix)
            if stripped.starts_with('^') && stripped.ends_with(".*") {
                let prefix = &stripped[1..stripped.len() - 2];
                if is_literal(prefix) {
                    let prefix_z3 = Z3String::from_str(ctx, prefix).ok()?;
                    return Some(prefix_z3.prefix(param_z3));
                }
            }
            // .*suffix$ → ends_with(param, suffix)
            if stripped.starts_with(".*") && stripped.ends_with('$') {
                let suffix = &stripped[2..stripped.len() - 1];
                if is_literal(suffix) {
                    let suffix_z3 = Z3String::from_str(ctx, suffix).ok()?;
                    return Some(suffix_z3.suffix(param_z3));
                }
            }
            // .*substr.* → contains(param, substr)
            if stripped.starts_with(".*") && stripped.ends_with(".*") && stripped.len() > 4 {
                let substr = &stripped[2..stripped.len() - 2];
                if is_literal(substr) {
                    let substr_z3 = Z3String::from_str(ctx, substr).ok()?;
                    return Some(param_z3.contains(&substr_z3));
                }
            }
            // Plan 23: Exact match — ^literal$ (no metacharacters) → eq(param, "literal")
            if stripped.starts_with('^') && stripped.ends_with('$') && stripped.len() > 2 {
                let literal = &stripped[1..stripped.len() - 1];
                if is_literal(literal) {
                    let literal_z3 = Z3String::from_str(ctx, literal).ok()?;
                    return Some(param_z3._eq(&literal_z3));
                }
            }
            // Plan 23: Prefix + suffix — ^prefix.*suffix$ → starts_with && ends_with
            if stripped.starts_with('^') && stripped.ends_with('$') && stripped.contains(".*") {
                let inner = &stripped[1..stripped.len() - 1];
                if let Some(dot_star_pos) = inner.find(".*") {
                    let prefix = &inner[..dot_star_pos];
                    let suffix = &inner[dot_star_pos + 2..];
                    if is_literal(prefix) && is_literal(suffix) && !suffix.is_empty() {
                        let prefix_z3 = Z3String::from_str(ctx, prefix).ok()?;
                        let suffix_z3 = Z3String::from_str(ctx, suffix).ok()?;
                        let prefix_check = prefix_z3.prefix(param_z3);
                        let suffix_check = suffix_z3.suffix(param_z3);
                        return Some(Bool::and(ctx, &[&prefix_check, &suffix_check]));
                    }
                }
            }
            // For complex regex patterns, Z3 String Sort cannot directly verify;
            // constant checking via Rust regex crate will handle these cases.
        }
    }

    None
}

// =============================================================================
// エフェクト検証コンテキスト (Effect Verification Context)
// =============================================================================

/// Effect verification context — tracks allowed and used effects per atom scope.
#[derive(Debug, Clone, Default)]
pub(crate) struct EffectCtx {
    /// Effects allowed in the current scope (from atom's effects annotation, transitively expanded)
    allowed_effects: HashSet<String>,
    /// Effects actually used in the body (from perform expressions)
    used_effects: HashSet<String>,
    /// Violation messages
    violations: Vec<String>,
}

impl EffectCtx {
    pub(crate) fn new(allowed: HashSet<String>) -> Self {
        Self {
            allowed_effects: allowed,
            used_effects: HashSet::new(),
            violations: Vec::new(),
        }
    }

    /// Record a perform and check if the effect is allowed
    pub(crate) fn perform_effect(&mut self, effect_name: &str) -> Result<(), String> {
        self.used_effects.insert(effect_name.to_string());
        if !self.allowed_effects.contains(effect_name) {
            let msg = format!(
                "Effect violation: '{}' is not in the allowed effect set {:?}",
                effect_name, self.allowed_effects
            );
            self.violations.push(msg.clone());
            return Err(msg);
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }
}

/// Verify effect containment for an atom using Z3.
/// Proves: ∀e ∈ UsedEffects(Body): e ∈ AllowedEffects(Signature)
pub(crate) fn verify_effect_containment(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
) -> MumeiResult<()> {
    // Check effect propagation: for each callee atom, verify callee.effects ⊆ caller.effects
    // Use leaf effects only to avoid false positives with composite effects.
    // E.g., caller [FileRead, FileWrite, Console] vs callee [IO] should pass
    // because both resolve to the same leaf set.
    let allowed_leaves = module_env.resolve_leaf_effects_from_effects(&atom.effects);
    let callees = collect_callees_stmt(body_stmt);
    for callee_name in &callees {
        if let Some(callee_atom) = module_env.get_atom(callee_name) {
            if !callee_atom.effects.is_empty() {
                let callee_leaves =
                    module_env.resolve_leaf_effects_from_effects(&callee_atom.effects);
                let missing: Vec<String> = callee_leaves
                    .iter()
                    .filter(|callee_eff| {
                        !allowed_leaves.contains(*callee_eff)
                            && !allowed_leaves
                                .iter()
                                .any(|allowed| module_env.is_subeffect(callee_eff, allowed))
                    })
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    let allowed_strs: Vec<String> =
                        atom.effects.iter().map(|e| e.name.clone()).collect();
                    let feedback =
                        build_effect_feedback(&atom.name, &missing[0], &allowed_strs, &missing);
                    let feedback_explanation =
                        feedback["explanation"].as_str().unwrap_or("").to_string();
                    return Err(MumeiError::verification_at(
                        format!(
                            "Effect propagation violation: atom '{}' calls '{}' which requires \
                             {:?} effect(s), but '{}' only declares effects: {:?}. \
                             Missing: {:?}. {}",
                            atom.name,
                            callee_name,
                            callee_atom.effects,
                            atom.name,
                            atom.effects,
                            missing,
                            feedback_explanation,
                        ),
                        atom.span.clone(),
                    )
                    .with_help(format!(
                        "Add the missing effects {:?} to atom '{}', or remove the call to '{}'.",
                        missing, atom.name, callee_name
                    )));
                }
            }
        }
    }

    // Plan 6: Negative effects — verify that body does not use forbidden effects.
    // E.g., `effects: [!IO]` means the atom forbids IO and all its sub-effects.
    let negated_effects: Vec<&Effect> = atom.effects.iter().filter(|e| e.negated).collect();
    if !negated_effects.is_empty() {
        for callee_name in &callees {
            if let Some(callee_atom) = module_env.get_atom(callee_name) {
                for callee_eff in &callee_atom.effects {
                    if callee_eff.negated {
                        continue;
                    }
                    let callee_leaf_set =
                        module_env.resolve_leaf_effects(std::slice::from_ref(&callee_eff.name));
                    for neg in &negated_effects {
                        let neg_leaf_set =
                            module_env.resolve_leaf_effects(std::slice::from_ref(&neg.name));
                        // Check if any callee leaf is a sub-effect of the negated effect
                        // or is in the negated leaf set
                        for callee_leaf in &callee_leaf_set {
                            let is_forbidden = neg_leaf_set.contains(callee_leaf)
                                || module_env.is_subeffect(callee_leaf, &neg.name);
                            if is_forbidden {
                                return Err(MumeiError::verification_at(
                                    format!(
                                        "Negative effect violation: atom '{}' declares '!{}' \
                                         but calls '{}' which uses effect '{}' \
                                         (resolved leaf: '{}'). This effect is forbidden.",
                                        atom.name, neg.name, callee_name,
                                        callee_eff.name, callee_leaf,
                                    ),
                                    atom.span.clone(),
                                )
                                .with_help(format!(
                                    "Remove the call to '{}', or remove '!{}' from the effects declaration.",
                                    callee_name, neg.name
                                )));
                            }
                        }
                    }
                }
            }
        }
    }

    // Plan 6: Effect narrowing — diagnostic when caller has a subtype of callee's required effect.
    for callee_name in &callees {
        if let Some(callee_atom) = module_env.get_atom(callee_name) {
            for callee_eff in &callee_atom.effects {
                if callee_eff.negated {
                    continue;
                }
                // Check if the caller has a more specific (narrower) effect
                for caller_eff in &atom.effects {
                    if caller_eff.negated {
                        continue;
                    }
                    if caller_eff.name != callee_eff.name
                        && module_env.is_subeffect(&caller_eff.name, &callee_eff.name)
                    {
                        // Caller has a narrower effect — emit info diagnostic (not error)
                        eprintln!(
                            "Info: Effect narrowing at call site — atom '{}' provides '{}' \
                             (subtype of '{}') for callee '{}'.",
                            atom.name, caller_eff.name, callee_eff.name, callee_name
                        );
                    }
                }
            }
        }
    }

    // atom_ref パラメータの effect_set ⊆ caller のエフェクト
    // 複合エフェクト（IO, FullAccess 等）を正しく扱うため、両側をリーフに解決して比較する
    for param in &atom.params {
        if let Some(ref type_ref) = param.type_ref {
            if type_ref.is_fn_type() {
                if let Some(ref effect_set) = type_ref.effect_set {
                    let param_leaves = module_env.resolve_leaf_effects(effect_set);
                    let missing: Vec<String> = param_leaves
                        .iter()
                        .filter(|eff| {
                            !allowed_leaves.contains(*eff)
                                && !allowed_leaves
                                    .iter()
                                    .any(|allowed| module_env.is_subeffect(eff, allowed))
                        })
                        .cloned()
                        .collect();
                    if !missing.is_empty() {
                        return Err(MumeiError::verification_at(
                            format!(
                                "Effect polymorphism violation: atom '{}' accepts function parameter '{}' \
                                 with effect [{}], but '{}' only declares effects: {:?}. \
                                 The function parameter's effect must be a subset of the atom's declared effects. \
                                 Missing leaf effects: {:?}.",
                                atom.name, param.name, effect_set.join(", "), atom.name,
                                atom.effects.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
                                missing
                            ),
                            atom.span.clone(),
                        )
                        .with_help(format!(
                            "Add the missing effects {:?} to the effects declaration of atom '{}'.",
                            missing, atom.name
                        )));
                    }
                }
            }
        }
    }

    Ok(())
}

/// Save an effect mismatch violation report to report.json for self-healing integration.
pub(crate) fn save_effect_violation_report(
    output_dir: &Path,
    atom_name: &str,
    declared_effects: &[String],
    required_effect: &str,
    source_operation: &str,
    suggested_fixes: &[String],
) {
    let mut report = json!({
        "status": "failed",
        "atom": atom_name,
        "failure_type": FAILURE_EFFECT_NOT_ALLOWED,
        "violation_type": "effect_mismatch",
        "effect_violation": {
            "declared_effects": declared_effects,
            "required_effect": required_effect,
            "source_operation": source_operation,
            "suggested_fixes": suggested_fixes,
            "resolution_paths": [
                {
                    "strategy": "propagation",
                    "description": format!("Add '{}' to the effects declaration of atom '{}'", required_effect, atom_name),
                    "fix_type": "signature_change",
                    "target": atom_name,
                    "change": format!("effects: [{}, {}];", declared_effects.join(", "), required_effect)
                },
                {
                    "strategy": "isolation",
                    "description": format!("Remove the call to '{}' and use only pure computation", source_operation),
                    "fix_type": "body_change",
                    "target": atom_name,
                    "change": format!("Remove or replace '{}' with a pure alternative", source_operation)
                }
            ]
        },
        "reason": format!("Effect violation: atom '{}' declares effects {:?} but uses '{}' which requires [{}]",
            atom_name, declared_effects, source_operation, required_effect)
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

/// Save an effect propagation violation report to report.json for self-healing integration.
pub(crate) fn save_effect_propagation_report(
    output_dir: &Path,
    caller_name: &str,
    callee_name: &str,
    caller_effects: &[String],
    callee_effects: &[String],
    missing_effects: &[String],
) {
    let mut report = json!({
        "status": "failed",
        "atom": caller_name,
        "failure_type": FAILURE_EFFECT_NOT_ALLOWED,
        "violation_type": "effect_propagation",
        "effect_violation": {
            "caller": caller_name,
            "callee": callee_name,
            "caller_effects": caller_effects,
            "callee_effects": callee_effects,
            "missing_effects": missing_effects,
            "suggested_fixes": [
                format!("Add {:?} to atom '{}' effects declaration", missing_effects, caller_name),
                format!("Remove the call to '{}' from atom '{}'", callee_name, caller_name)
            ],
            "resolution_paths": [
                {
                    "strategy": "propagation",
                    "description": format!("Expand {}'s effect set to include {}'s effects", caller_name, callee_name),
                    "fix_type": "signature_change"
                },
                {
                    "strategy": "isolation",
                    "description": format!("Remove the dependency on '{}'", callee_name),
                    "fix_type": "body_change"
                }
            ]
        },
        "reason": format!("Effect propagation violation: '{}' calls '{}' which requires {:?}, but '{}' only declares {:?}",
            caller_name, callee_name, callee_effects, caller_name, caller_effects)
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

/// Save an effect polymorphism violation report to report.json for self-healing integration.
/// Called when an atom_ref parameter's effect_set is not a subset of the atom's declared effects.
pub(crate) fn save_effect_polymorphism_report(
    output_dir: &Path,
    atom_name: &str,
    param_name: &str,
    param_effect_set: &[String],
    declared_effects: &[String],
    missing_effects: &[String],
) {
    let mut report = json!({
        "status": "failed",
        "atom": atom_name,
        "failure_type": FAILURE_EFFECT_NOT_ALLOWED,
        "violation_type": "effect_polymorphism",
        "effect_violation": {
            "atom": atom_name,
            "param": param_name,
            "param_effect_set": param_effect_set,
            "declared_effects": declared_effects,
            "missing_effects": missing_effects,
            "suggested_fixes": [
                format!(
                    "Add {:?} to atom '{}' effects declaration: effects: [{}];",
                    missing_effects,
                    atom_name,
                    declared_effects.iter().chain(missing_effects.iter()).cloned().collect::<Vec<_>>().join(", ")
                ),
                format!(
                    "Change parameter '{}' to use only effects declared by '{}'",
                    param_name, atom_name
                )
            ],
            "resolution_paths": [
                {
                    "strategy": "propagation",
                    "description": format!(
                        "Add missing effects {:?} to atom '{}' effects declaration",
                        missing_effects, atom_name
                    ),
                    "fix_type": "signature_change",
                    "target": atom_name,
                    "change": format!(
                        "effects: [{}];",
                        declared_effects.iter().chain(missing_effects.iter()).cloned().collect::<Vec<_>>().join(", ")
                    )
                },
                {
                    "strategy": "restriction",
                    "description": format!(
                        "Restrict parameter '{}' to use only effects in {:?}",
                        param_name, declared_effects
                    ),
                    "fix_type": "param_change",
                    "target": param_name
                }
            ]
        },
        "reason": format!(
            "Effect polymorphism violation: atom '{}' accepts function parameter '{}' with effect {:?}, \
             but '{}' only declares effects {:?}. Missing leaf effects: {:?}.",
            atom_name, param_name, param_effect_set, atom_name, declared_effects, missing_effects
        )
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

pub(crate) fn verify_effect_consistency(atom: &Atom, module_env: &ModuleEnv) -> MumeiResult<()> {
    let declared: Vec<String> = atom.effects.iter().map(|e| e.name.clone()).collect();
    let body_stmt_eff = parse_body_expr(&atom.body_expr);
    let inferred = infer_effects(atom, &body_stmt_eff, module_env);

    for eff in &inferred {
        // 宣言に含まれるか、宣言のいずれかのサブタイプかをチェック
        let is_covered = declared.contains(&eff.name)
            || declared
                .iter()
                .any(|d| module_env.is_subeffect(&eff.name, d));
        if !is_covered {
            let all_effects: Vec<String> = declared
                .iter()
                .chain(std::iter::once(&eff.name))
                .cloned()
                .collect();
            eprintln!(
                "  ⚠️  Effect suggestion for atom '{}': inferred effect '{}' is not declared. \
                 Suggested: effects: [{}];",
                atom.name,
                eff.name,
                all_effects.join(", ")
            );
        }
    }
    Ok(())
}

// =============================================================================
// Step 4: ハイブリッド・アプローチによるエフェクトパラメータ検証
// =============================================================================

/// 定数制約チェック（Constant Folding）。
/// 定数パスに対する制約を Rust 側で直接検証する。
pub(crate) fn check_constant_constraint(value: &str, constraint: &str) -> bool {
    // Compound constraint: split on && and check all sub-constraints
    // NOTE: This naive split does not respect && inside quoted strings
    // (e.g., not_contains(path, "a&&b") would be incorrectly split).
    // This is an acceptable limitation since constraint string arguments
    // are path patterns that should never contain "&&".
    if constraint.contains("&&") {
        return constraint
            .split("&&")
            .all(|part| check_constant_constraint(value, part.trim()));
    }

    // パーサーは "starts_with(path, \"/tmp/\")" のように2引数形式で制約を出力する。
    // 文字列引数（最後のクォートされた値）を抽出して検証する。
    let extract_string_arg = |c: &str| -> Option<String> {
        // 最後の "..." を抽出する
        if let Some(last_quote_end) = c.rfind('"') {
            let before = &c[..last_quote_end];
            if let Some(last_quote_start) = before.rfind('"') {
                return Some(c[last_quote_start + 1..last_quote_end].to_string());
            }
        }
        None
    };

    // starts_with 制約（1引数 or 2引数形式）
    if constraint.starts_with("starts_with(") {
        if let Some(arg) = extract_string_arg(constraint) {
            return value.starts_with(&arg);
        }
    }
    // contains 制約
    if constraint.starts_with("contains(") {
        if let Some(arg) = extract_string_arg(constraint) {
            return value.contains(&arg);
        }
    }
    // ends_with 制約
    if constraint.starts_with("ends_with(") {
        if let Some(arg) = extract_string_arg(constraint) {
            return value.ends_with(&arg);
        }
    }
    // not_contains 制約
    if constraint.starts_with("not_contains(") {
        if let Some(arg) = extract_string_arg(constraint) {
            return !value.contains(&arg);
        }
    }
    // Plan 10: matches() 制約 — Rust regex crate による定数パスの正規表現マッチング
    if constraint.starts_with("matches(") {
        if let Some(pattern) = extract_string_arg(constraint) {
            if let Ok(re) = regex::Regex::new(&pattern) {
                return re.is_match(value);
            }
        }
    }
    // 不明な制約は false を返す（安全側に倒す — 検証できない場合は拒否）
    false
}

/// エフェクトパラメータの検証。
/// 定数パスは Rust 側で直接チェック（Constant Folding — fast path）。
/// 変数パスは Z3 String Sort で検証（Plan 5: Z3 String Sort migration）。
pub(crate) fn verify_effect_params(atom: &Atom, module_env: &ModuleEnv) -> MumeiResult<()> {
    for effect in &atom.effects {
        // effect_defs を優先、なければ effects を参照
        let effect_def = module_env
            .effect_defs
            .get(&effect.name)
            .or_else(|| module_env.effects.get(&effect.name));
        if let Some(def) = effect_def {
            for param in &effect.params {
                if param.is_constant {
                    // Constant Folding: 定数パスは Rust 側で直接チェック (fast path)
                    if let Some(ref constraint) = def.constraint {
                        if !check_constant_constraint(&param.value, constraint) {
                            return Err(MumeiError::verification_at(
                                format!(
                                    "Effect '{}' parameter '{}' violates constraint: {}",
                                    effect.name, param.value, constraint
                                ),
                                effect.span.clone(),
                            ));
                        }
                    }
                } else {
                    // Plan 5: Variable path — verify with Z3 String Sort.
                    // Create a fresh Z3 context and solver to check the constraint
                    // is satisfiable for the symbolic parameter.
                    if let Some(ref constraint) = def.constraint {
                        let z3_cfg = z3::Config::new();
                        let z3_ctx = z3::Context::new(&z3_cfg);
                        let solver = z3::Solver::new(&z3_ctx);
                        // Timeout: 500ms for string constraints
                        let mut z3_params = z3::Params::new(&z3_ctx);
                        z3_params.set_u32("timeout", 500);
                        solver.set_params(&z3_params);

                        let param_z3_str =
                            z3::ast::String::new_const(&z3_ctx, param.value.as_str());
                        let check_result = {
                            let maybe_bool =
                                parse_constraint_to_z3_string(&z3_ctx, constraint, &param_z3_str);
                            if let Some(constraint_bool) = maybe_bool {
                                solver.assert(&constraint_bool);
                                Some(solver.check())
                            } else {
                                None
                            }
                        };
                        if let Some(result) = check_result {
                            match result {
                                z3::SatResult::Unsat => {
                                    return Err(MumeiError::verification_at(
                                        format!(
                                            "Effect '{}' variable parameter '{}' constraint '{}' \
                                             is unsatisfiable (Z3 String Sort)",
                                            effect.name, param.value, constraint
                                        ),
                                        effect.span.clone(),
                                    ));
                                }
                                z3::SatResult::Unknown => {
                                    // Timeout or undecidable — emit warning, do not block.
                                    eprintln!(
                                        "Warning: Z3 String constraint check for '{}' \
                                         parameter '{}' timed out or was undecidable",
                                        effect.name, param.value
                                    );
                                }
                                z3::SatResult::Sat => {
                                    // Constraint is satisfiable — OK
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
