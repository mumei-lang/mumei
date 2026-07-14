use super::module_env::*;
use super::*;

#[derive(Debug, Clone)]
pub struct ConstraintMapping {
    pub(crate) param_name: String,
    pub(crate) type_name: Option<String>,
    pub(crate) base_type: String,
    pub(crate) predicate_raw: String,
    pub(crate) span: Span,
}

/// A single step in the data flow chain for expression-level tracking.
#[derive(Debug, Clone)]
pub struct DataFlowEntry {
    pub step: String,
    pub line: u32,
    pub col: u32,
    pub description: String,
    pub constraint: Option<String>,
}

// =============================================================================
// Failure Type Classification (Feature 1-d)
// =============================================================================

/// Classification of verification failure types for structured reporting.
pub const FAILURE_POSTCONDITION_VIOLATED: &str = "postcondition_violated";
pub const FAILURE_PRECONDITION_VIOLATED: &str = "precondition_violated";
pub const FAILURE_DIVISION_BY_ZERO: &str = "division_by_zero";
pub const FAILURE_TRAIT_LAW_VIOLATED: &str = "trait_law_violated";
pub const FAILURE_LINEARITY_VIOLATED: &str = "linearity_violated";
pub const FAILURE_INVARIANT_VIOLATED: &str = "invariant_violated";
pub const FAILURE_EXHAUSTIVENESS_FAILED: &str = "exhaustiveness_failed";
pub const FAILURE_RESOURCE_CONFLICT: &str = "resource_conflict";
pub const ENABLE_RECONSTRUCTION_LOSS_ENV: &str = "ENABLE_RECONSTRUCTION_LOSS";
pub const ENABLE_SELF_CORRECTION_ENV: &str = "ENABLE_SELF_CORRECTION";

pub fn reconstruction_loss_output_enabled() -> bool {
    env_flag_enabled(ENABLE_RECONSTRUCTION_LOSS_ENV)
}

pub fn self_correction_enabled() -> bool {
    env_flag_enabled(ENABLE_SELF_CORRECTION_ENV)
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            let normalized = value.trim().to_ascii_lowercase();
            !normalized.is_empty() && normalized != "0" && normalized != "false"
        })
        .unwrap_or(false)
}

// =============================================================================
// Suggestion Templates per Failure Type (Feature 1-e)
// =============================================================================

pub fn suggestion_for_failure_type(failure_type: &str) -> &'static str {
    match failure_type {
        FAILURE_POSTCONDITION_VIOLATED => {
            "Ensure the body's return value satisfies the ensures clause, or relax the ensures constraint \
             (本体の戻り値が ensures 句を満たすようにするか、ensures 制約を緩和してください)"
        }
        FAILURE_PRECONDITION_VIOLATED => {
            "The caller must establish the callee's requires clause before the call \
             (呼び出し元は呼び出し先の requires 句を呼び出し前に確立する必要があります)"
        }
        FAILURE_DIVISION_BY_ZERO => {
            "Add a guard condition `divisor != 0` in the requires clause / \
             requires 句に `divisor != 0` ガード条件を追加してください"
        }
        FAILURE_TRAIT_LAW_VIOLATED => {
            "The trait implementation does not satisfy the algebraic law; review the impl body \
             (トレイト実装が代数法則を満たしていません。impl 本体を見直してください)"
        }
        FAILURE_LINEARITY_VIOLATED => {
            "Clone the value before the second use, or restructure to avoid reuse \
             (2回目の使用前に値をクローンするか、再利用を避けるよう構造を変更してください)"
        }
        FAILURE_INVARIANT_VIOLATED => {
            "The loop/recursive invariant is not maintained; strengthen the invariant or fix the body \
             (ループ/再帰不変条件が維持されていません。不変条件を強化するか本体を修正してください)"
        }
        FAILURE_EXHAUSTIVENESS_FAILED => {
            "Not all cases are covered in the match expression; add missing patterns \
             (match 式で全てのケースがカバーされていません。不足パターンを追加してください)"
        }
        FAILURE_RESOURCE_CONFLICT => {
            "Resource acquisition order may cause deadlock; reorder acquire calls to follow priority ordering \
             (リソース取得順序がデッドロックを引き起こす可能性があります。優先順位に従って取得順序を変更してください)"
        }
        FAILURE_EFFECT_NOT_ALLOWED => {
            "Add the required effect to the atom's effect list or the security policy / \
             必要なエフェクトを atom のエフェクトリストまたはセキュリティポリシーに追加してください"
        }
        _ => "Review the verification failure and adjust the code or contracts accordingly \
              (検証失敗を確認し、コードまたは契約を適宜修正してください)",
    }
}

/// Constant for effect-not-allowed failure type
pub const FAILURE_EFFECT_NOT_ALLOWED: &str = "effect_not_allowed";

// =============================================================================
// Contextual Suggestion Generation (Dynamic)
// =============================================================================

/// Build a contextual suggestion using failure_type, counterexample, and
/// structured_unsat_core. Falls back to `suggestion_for_failure_type()` when
/// the inputs are insufficient for a more specific message.
pub fn build_contextual_suggestion(
    failure_type: &str,
    counterexample: Option<&serde_json::Value>,
    structured_unsat_core: Option<&serde_json::Value>,
) -> String {
    if failure_type.is_empty() {
        return "Verification passed; no fix is required.".to_string();
    }
    let ce_map = counterexample.and_then(|ce| ce.as_object());

    // Try to extract a violated constraint description from unsat core
    let violated_constraint = structured_unsat_core
        .and_then(|core| core.as_array())
        .and_then(|arr| {
            arr.iter().find_map(|entry| {
                entry
                    .get("description")
                    .and_then(|d| d.as_str())
                    .map(|s| s.to_string())
            })
        });

    match failure_type {
        FAILURE_PRECONDITION_VIOLATED => {
            if let Some(ce) = ce_map {
                // Find which parameter(s) have problematic values
                let bindings: Vec<String> = ce
                    .iter()
                    .map(|(k, v)| {
                        let val = v
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| v.to_string());
                        format!("{} = {}", k, val)
                    })
                    .collect();
                let bindings_str = if bindings.is_empty() {
                    String::new()
                } else {
                    format!(" (counterexample: {})", bindings.join(", "))
                };
                if let Some(ref vc) = violated_constraint {
                    format!(
                        "Add a precondition to guard against this case: '{}'{} \
                         (この場合を防ぐ事前条件を追加してください: '{}'{})",
                        vc, bindings_str, vc, bindings_str
                    )
                } else {
                    // Try to infer from counterexample values
                    let param_hints: Vec<String> = ce
                        .iter()
                        .filter(|(_, v)| {
                            v.as_str().map(|s| s == "0").unwrap_or(false)
                                || v.as_i64().map(|n| n == 0).unwrap_or(false)
                        })
                        .map(|(k, _)| format!("{} != 0", k))
                        .collect();
                    if !param_hints.is_empty() {
                        format!(
                            "Add 'requires: {}' to the atom's precondition \
                             (atom の事前条件に 'requires: {}' を追加してください)",
                            param_hints.join(" && "),
                            param_hints.join(" && ")
                        )
                    } else {
                        suggestion_for_failure_type(failure_type).to_string()
                    }
                }
            } else {
                suggestion_for_failure_type(failure_type).to_string()
            }
        }
        FAILURE_POSTCONDITION_VIOLATED => {
            if let Some(ce) = ce_map {
                let bindings: Vec<String> = ce
                    .iter()
                    .map(|(k, v)| {
                        let val = v
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| v.to_string());
                        format!("{} = {}", k, val)
                    })
                    .collect();
                let bindings_str = bindings.join(", ");
                format!(
                    "The ensures clause is not satisfied when {}. \
                     Adjust the body or relax ensures \
                     ({} のとき ensures 句が満たされません。\
                     本体を修正するか ensures を緩和してください)",
                    bindings_str, bindings_str
                )
            } else {
                suggestion_for_failure_type(failure_type).to_string()
            }
        }
        FAILURE_DIVISION_BY_ZERO => {
            if let Some(ce) = ce_map {
                // Find the zero-valued parameter
                let zero_params: Vec<&String> = ce
                    .iter()
                    .filter(|(_, v)| {
                        v.as_str().map(|s| s == "0").unwrap_or(false)
                            || v.as_i64().map(|n| n == 0).unwrap_or(false)
                    })
                    .map(|(k, _)| k)
                    .collect();
                if !zero_params.is_empty() {
                    let guards: Vec<String> =
                        zero_params.iter().map(|p| format!("{} != 0", p)).collect();
                    format!(
                        "Add 'requires: {}' to prevent division by zero \
                         (ゼロ除算を防ぐため 'requires: {}' を追加してください)",
                        guards.join(" && "),
                        guards.join(" && ")
                    )
                } else {
                    suggestion_for_failure_type(failure_type).to_string()
                }
            } else {
                suggestion_for_failure_type(failure_type).to_string()
            }
        }
        FAILURE_INVARIANT_VIOLATED => {
            if let Some(ref vc) = violated_constraint {
                format!(
                    "The invariant is violated because of constraint: '{}'. \
                     Strengthen the invariant or fix the loop body \
                     (制約 '{}' により不変条件が破られています。\
                     不変条件を強化するかループ本体を修正してください)",
                    vc, vc
                )
            } else {
                suggestion_for_failure_type(failure_type).to_string()
            }
        }
        _ => suggestion_for_failure_type(failure_type).to_string(),
    }
}

// =============================================================================
// Compound Constraint Decomposition
// =============================================================================

/// Splits `&&`-joined constraints at the top level while respecting parenthesis
/// depth and quoted strings.
///
/// Example: `starts_with(path, "/tmp/") && not_contains(path, "..")`
/// → `["starts_with(path, \"/tmp/\")", "not_contains(path, \"..\")"]`
pub fn split_compound_constraint(predicate_raw: &str) -> Vec<String> {
    let pred = predicate_raw.trim();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth: i32 = 0;
    let mut in_quotes = false;
    let chars: Vec<char> = pred.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        let ch = chars[i];
        if ch == '"' && (i == 0 || chars[i - 1] != '\\') {
            in_quotes = !in_quotes;
            current.push(ch);
            i += 1;
            continue;
        }
        if in_quotes {
            current.push(ch);
            i += 1;
            continue;
        }
        if ch == '(' {
            depth += 1;
            current.push(ch);
            i += 1;
            continue;
        }
        if ch == ')' {
            depth -= 1;
            current.push(ch);
            i += 1;
            continue;
        }
        // Only split on `&&` when depth == 0 and not inside quotes
        if depth == 0 && ch == '&' && i + 1 < len && chars[i + 1] == '&' {
            let trimmed = current.trim().to_string();
            if !trimmed.is_empty() {
                parts.push(trimmed);
            }
            current.clear();
            i += 2; // skip both '&' characters
            continue;
        }
        current.push(ch);
        i += 1;
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        parts.push(trimmed);
    }
    parts
}

/// Evaluate a single sub-constraint against a concrete counterexample value.
/// Checks starts_with/ends_with/contains/not_contains/range patterns.
pub fn evaluate_sub_constraint(sub_pred: &str, value: &str) -> bool {
    let trimmed = sub_pred.trim();

    // starts_with(param, "prefix")
    if let Some(inner) = trimmed.strip_prefix("starts_with(") {
        if let Some(inner) = inner.strip_suffix(')') {
            if let Some((_p, rest)) = inner.split_once(',') {
                let prefix = rest.trim().trim_matches('"');
                return value.trim_matches('"').starts_with(prefix);
            }
        }
    }

    // ends_with(param, "suffix")
    if let Some(inner) = trimmed.strip_prefix("ends_with(") {
        if let Some(inner) = inner.strip_suffix(')') {
            if let Some((_p, rest)) = inner.split_once(',') {
                let suffix = rest.trim().trim_matches('"');
                return value.trim_matches('"').ends_with(suffix);
            }
        }
    }

    // not_contains(param, "substr")
    if let Some(inner) = trimmed.strip_prefix("not_contains(") {
        if let Some(inner) = inner.strip_suffix(')') {
            if let Some((_p, rest)) = inner.split_once(',') {
                let substr = rest.trim().trim_matches('"');
                return !value.trim_matches('"').contains(substr);
            }
        }
    }

    // contains(param, "substr")
    if let Some(inner) = trimmed.strip_prefix("contains(") {
        if let Some(inner) = inner.strip_suffix(')') {
            if let Some((_p, rest)) = inner.split_once(',') {
                let substr = rest.trim().trim_matches('"');
                return value.trim_matches('"').contains(substr);
            }
        }
    }

    // Simple comparison patterns: v >= N, v <= N, v > N, v < N, v != N
    // Try to parse as numeric comparison
    #[allow(clippy::type_complexity)]
    let comparisons: Vec<(&str, fn(i64, i64) -> bool)> = vec![
        (">=", (|a, b| a >= b) as fn(i64, i64) -> bool),
        ("<=", |a, b| a <= b),
        ("!=", |a, b| a != b),
        (">", |a, b| a > b),
        ("<", |a, b| a < b),
        ("==", |a, b| a == b),
    ];
    for (cmp_op, cmp_fn) in &comparisons {
        if let Some(idx) = trimmed.find(cmp_op) {
            let rhs = trimmed[idx + cmp_op.len()..].trim();
            if let (Ok(val_num), Ok(rhs_num)) =
                (value.trim_matches('"').parse::<i64>(), rhs.parse::<i64>())
            {
                return cmp_fn(val_num, rhs_num);
            }
        }
    }

    // Unknown sub-constraint — conservatively return true
    true
}

// =============================================================================
// Natural Language Constraint Template Engine (Feature 1-b)
// =============================================================================

/// Pattern-matches common predicate forms and generates human/AI-readable descriptions.
/// Returns bilingual output: English primary, Japanese in parentheses.
/// Compound constraints (joined by `&&`) are decomposed and each sub-constraint
/// is individually explained with numbered prefixes.
pub fn constraint_to_natural_language(
    param_name: &str,
    type_name: &str,
    predicate_raw: &str,
    value: &str,
) -> String {
    let pred = predicate_raw.trim();

    // Compound constraint decomposition: if multiple sub-constraints, explain each individually
    let sub_parts = split_compound_constraint(pred);
    if sub_parts.len() > 1 {
        let total = sub_parts.len();
        let explanations: Vec<String> = sub_parts
            .iter()
            .enumerate()
            .map(|(i, sub)| {
                let sub_explanation =
                    constraint_to_natural_language(param_name, type_name, sub, value);
                format!("[{}/{}] {}", i + 1, total, sub_explanation)
            })
            .collect();
        return explanations.join(" AND ");
    }

    // Try to match range pattern: v >= N && v <= M  or  N <= v && v <= M
    if let Some(range_desc) = try_match_range(pred, param_name, type_name, value) {
        return range_desc;
    }

    // Modulo constraints: v % N == 0
    if let Some(desc) = try_match_modulo(pred, param_name, value) {
        return desc;
    }

    // Enum/set membership: v == 1 || v == 2 || v == 3
    if let Some(desc) = try_match_enum(pred, param_name, value) {
        return desc;
    }

    // String constraints: starts_with, ends_with, contains
    if let Some(desc) = try_match_string_constraint(pred, param_name, value) {
        return desc;
    }

    // Negation patterns: !(expr) or v != N
    if let Some(desc) = try_match_negation(pred, param_name, type_name, value) {
        return desc;
    }

    // Single comparison patterns
    if let Some(desc) = try_match_comparison(pred, param_name, type_name, value) {
        return desc;
    }

    // Fallback for unrecognized patterns
    format!(
        "{param} must satisfy constraint '{pred}' but value is {val} \
         ({param} は制約 '{pred}' を満たす必要がありますが、値は {val} です)",
        param = param_name,
        pred = predicate_raw,
        val = value,
    )
}

/// Try to match a range pattern like "v >= N && v <= M" or reversed "N <= v && v <= M"
pub(crate) fn try_match_range(
    pred: &str,
    param_name: &str,
    type_name: &str,
    value: &str,
) -> Option<String> {
    let parts: Vec<&str> = pred.split("&&").map(|s| s.trim()).collect();
    if parts.len() != 2 {
        return None;
    }
    // Try normal order: v >= N && v <= M
    let lower = extract_bound(parts[0], true).or_else(|| extract_bound_reversed(parts[0], true));
    let upper = extract_bound(parts[1], false).or_else(|| extract_bound_reversed(parts[1], false));
    let (lower, upper) = match (lower, upper) {
        (Some(l), Some(u)) => (l, u),
        _ => return None,
    };
    Some(format!(
        "{param} is {val}, which violates {ty} constraint ({lower_bound} to {upper_bound}) \
         ({param} が {val} のとき、{ty} の制約 {lower_bound} 以上 {upper_bound} 以下を逸脱します)",
        param = param_name,
        val = value,
        ty = type_name,
        lower_bound = lower,
        upper_bound = upper,
    ))
}

/// Extract a numeric bound from a comparison expression
pub(crate) fn extract_bound(expr: &str, is_lower: bool) -> Option<String> {
    let trimmed = expr.trim();
    let ops: &[&str] = if is_lower { &[">=", ">"] } else { &["<=", "<"] };
    for op in ops {
        if let Some(idx) = trimmed.find(op) {
            let rhs = trimmed[idx + op.len()..].trim();
            return Some(rhs.to_string());
        }
    }
    None
}

/// Extract a numeric bound from reversed comparison: "N <= v" (lower) or "N >= v" (upper)
pub(crate) fn extract_bound_reversed(expr: &str, is_lower: bool) -> Option<String> {
    let trimmed = expr.trim();
    // For lower bound, look for "N <= v" pattern (reversed)
    let ops: &[&str] = if is_lower { &["<=", "<"] } else { &[">=", ">"] };
    for op in ops {
        if let Some(idx) = trimmed.find(op) {
            let lhs = trimmed[..idx].trim();
            // lhs should be a numeric literal (the bound), not a variable name
            if !lhs.is_empty()
                && lhs
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '-' || c == '.')
            {
                return Some(lhs.to_string());
            }
        }
    }
    None
}

/// Try to match modulo patterns: v % N == 0
pub(crate) fn try_match_modulo(pred: &str, param_name: &str, value: &str) -> Option<String> {
    let re_parts: Vec<&str> = pred.split("==").map(|s| s.trim()).collect();
    if re_parts.len() != 2 {
        return None;
    }
    // Check for pattern: v % N == 0 or 0 == v % N
    let (mod_part, zero_part) = if re_parts[0].contains('%') {
        (re_parts[0], re_parts[1])
    } else if re_parts[1].contains('%') {
        (re_parts[1], re_parts[0])
    } else {
        return None;
    };
    if zero_part != "0" {
        return None;
    }
    let mod_parts: Vec<&str> = mod_part.split('%').map(|s| s.trim()).collect();
    if mod_parts.len() != 2 {
        return None;
    }
    let divisor = mod_parts[1];
    Some(format!(
        "'{param}' must be a multiple of {n} but value is {val} \
         ('{param}' は {n} の倍数である必要がありますが、値は {val} です)",
        param = param_name,
        n = divisor,
        val = value,
    ))
}

/// Try to match enum/set membership: v == 1 || v == 2 || v == 3
pub(crate) fn try_match_enum(pred: &str, param_name: &str, value: &str) -> Option<String> {
    let parts: Vec<&str> = pred.split("||").map(|s| s.trim()).collect();
    if parts.len() < 2 {
        return None;
    }
    let mut values = Vec::new();
    for part in &parts {
        let eq_parts: Vec<&str> = part.split("==").map(|s| s.trim()).collect();
        if eq_parts.len() != 2 {
            return None;
        }
        // One side should be a variable-like token, other should be a value
        let val = if eq_parts[0].chars().all(|c| c.is_ascii_digit() || c == '-') {
            eq_parts[0]
        } else if eq_parts[1].chars().all(|c| c.is_ascii_digit() || c == '-') {
            eq_parts[1]
        } else {
            return None;
        };
        values.push(val.to_string());
    }
    Some(format!(
        "'{param}' must be one of [{vals}] but value is {val} \
         ('{param}' は [{vals}] のいずれかである必要がありますが、値は {val} です)",
        param = param_name,
        vals = values.join(", "),
        val = value,
    ))
}

/// Try to match string constraint patterns: starts_with, ends_with, contains
pub(crate) fn try_match_string_constraint(
    pred: &str,
    param_name: &str,
    value: &str,
) -> Option<String> {
    let string_fns = [
        ("starts_with", "must start with", "で始まる必要がありますが"),
        ("ends_with", "must end with", "で終わる必要がありますが"),
        ("contains", "must contain", "を含む必要がありますが"),
    ];
    for (fn_name, en_desc, ja_desc) in &string_fns {
        if let Some(start) = pred.find(fn_name) {
            // Extract the argument: starts_with(var, "prefix") or starts_with(var, prefix)
            let after = &pred[start + fn_name.len()..];
            if let Some(paren_start) = after.find('(') {
                let inner = &after[paren_start + 1..];
                if let Some(paren_end) = inner.rfind(')') {
                    let args_str = &inner[..paren_end];
                    let args: Vec<&str> = args_str.splitn(2, ',').map(|s| s.trim()).collect();
                    let pattern_val = if args.len() == 2 {
                        args[1].trim_matches('"').trim_matches('\'')
                    } else if args.len() == 1 {
                        args[0].trim_matches('"').trim_matches('\'')
                    } else {
                        continue;
                    };
                    return Some(format!(
                        "'{param}' {en} \"{pattern}\" but value is {val} \
                         ('{param}' は \"{pattern}\" {ja}、値は {val} です)",
                        param = param_name,
                        en = en_desc,
                        pattern = pattern_val,
                        ja = ja_desc,
                        val = value,
                    ));
                }
            }
        }
    }
    None
}

/// Try to match negation patterns: !(expr) or v != N
pub(crate) fn try_match_negation(
    pred: &str,
    param_name: &str,
    type_name: &str,
    value: &str,
) -> Option<String> {
    let trimmed = pred.trim();
    // Pattern: !(inner_expr)
    if trimmed.starts_with("!(") && trimmed.ends_with(')') {
        let inner = &trimmed[2..trimmed.len() - 1];
        return Some(format!(
            "'{param}' must NOT satisfy '{inner}' but value is {val} \
             ('{param}' は '{inner}' を満たしてはなりませんが、値は {val} です)",
            param = param_name,
            inner = inner,
            val = value,
        ));
    }
    // Pattern: v != N (already handled in try_match_comparison, but handle standalone)
    if let Some(idx) = trimmed.find("!=") {
        let rhs = trimmed[idx + 2..].trim();
        if !rhs.is_empty() {
            return Some(format!(
                "'{param}' ({ty}) must not be {rhs} but value is {val} \
                 ('{param}' ({ty}) は {rhs} であってはなりませんが、値は {val} です)",
                param = param_name,
                ty = type_name,
                rhs = rhs,
                val = value,
            ));
        }
    }
    None
}

/// Try to match single comparison patterns
pub(crate) fn try_match_comparison(
    pred: &str,
    param_name: &str,
    type_name: &str,
    value: &str,
) -> Option<String> {
    // Ordered from most specific to least specific operator
    let patterns: &[(&str, &str, &str)] = &[
        (">=", "must be at least", "以上である必要がありますが"),
        ("<=", "must be at most", "以下である必要がありますが"),
        ("!=", "must not be", "であってはなりませんが"),
        (">", "must be greater than", "より大きい必要がありますが"),
        ("<", "must be less than", "未満である必要がありますが"),
    ];

    for (op, en_desc, ja_desc) in patterns {
        // Look for the operator in the predicate (e.g., "v <= 120")
        if let Some(idx) = pred.find(op) {
            let rhs = pred[idx + op.len()..].trim();
            // Only match if rhs looks like a number or simple identifier
            if rhs.is_empty() {
                continue;
            }
            return Some(format!(
                "{param} is {val}, which violates {ty} constraint {pred} \
                 ({param} {en} {rhs} ({param} は {rhs} {ja}、値は {val} です))",
                param = param_name,
                val = value,
                ty = type_name,
                pred = pred,
                rhs = rhs,
                en = en_desc,
                ja = ja_desc,
            ));
        }
    }
    None
}

/// Build constraint mappings for an atom's parameters by looking up their refined types.
pub fn build_constraint_mappings_for_atom(
    atom: &Atom,
    module_env: &ModuleEnv,
) -> Vec<ConstraintMapping> {
    let mut mappings = Vec::new();
    for param in &atom.params {
        if let Some(ref type_ref) = param.type_ref {
            let type_name_str = &type_ref.name;
            if let Some(refined) = module_env.get_type(type_name_str) {
                mappings.push(ConstraintMapping {
                    param_name: param.name.clone(),
                    type_name: Some(type_name_str.clone()),
                    base_type: refined._base_type.clone(),
                    predicate_raw: refined.predicate_raw.clone(),
                    span: refined.span.clone(),
                });
            }
        }
    }
    mappings
}

/// Build semantic feedback JSON from constraint mappings and counterexample values.
pub fn build_semantic_feedback(
    constraint_mappings: &[ConstraintMapping],
    counterexample: Option<&serde_json::Value>,
    atom: &Atom,
    failure_type: &str,
    data_flow_entries: Option<&[DataFlowEntry]>,
) -> Option<serde_json::Value> {
    let ce_map = counterexample.and_then(|ce| ce.as_object());
    let mut violated_constraints = Vec::new();

    for mapping in constraint_mappings {
        let value = ce_map
            .and_then(|m| m.get(&mapping.param_name))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let type_name = mapping.type_name.as_deref().unwrap_or(&mapping.base_type);
        let explanation = constraint_to_natural_language(
            &mapping.param_name,
            type_name,
            &mapping.predicate_raw,
            value,
        );

        let mut vc_entry = json!({
            "param": mapping.param_name,
            "type": type_name,
            "value": value,
            "constraint": mapping.predicate_raw,
            "explanation": explanation,
            "suggestion": build_contextual_suggestion(failure_type, counterexample, None)
        });

        // Compound constraint decomposition: add sub_constraints array
        let sub_parts = split_compound_constraint(&mapping.predicate_raw);
        if sub_parts.len() > 1 {
            let sub_constraints: Vec<serde_json::Value> = sub_parts
                .iter()
                .enumerate()
                .map(|(idx, sub)| {
                    let satisfied = evaluate_sub_constraint(sub, value);
                    let sub_explanation =
                        constraint_to_natural_language(&mapping.param_name, type_name, sub, value);
                    json!({
                        "index": idx,
                        "raw": sub,
                        "satisfied": satisfied,
                        "explanation": sub_explanation
                    })
                })
                .collect();
            vc_entry["sub_constraints"] = json!(sub_constraints);
        }

        violated_constraints.push(vc_entry);
    }

    if violated_constraints.is_empty() && ce_map.is_none() {
        return None;
    }

    let mut feedback = json!({
        "violated_constraints": violated_constraints
    });

    // Add data_flow if available
    if let Some(entries) = data_flow_entries {
        let data_flow: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                let mut entry = json!({
                    "step": e.step,
                    "line": e.line,
                    "col": e.col,
                    "description": e.description
                });
                if let Some(ref c) = e.constraint {
                    entry["constraint"] = json!(c);
                }
                entry
            })
            .collect();
        feedback["data_flow"] = json!(data_flow);
    }

    // Add context about the atom's contracts
    feedback["context"] = json!({
        "requires": atom.requires,
        "ensures": atom.ensures,
        "body_expr": atom.body_expr
    });

    Some(feedback)
}

/// Build the P9-E Loss Vector JSON for a verification failure.
pub fn build_loss_vector(
    atom: &Atom,
    failure_type: &str,
    counterexample: Option<&serde_json::Value>,
    span: &Span,
) -> serde_json::Value {
    let counter_example = counterexample
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));

    let base_instruction = build_contextual_suggestion(failure_type, counterexample, None);
    let feedback_instruction = if self_correction_enabled() {
        build_self_correction_feedback_instruction(atom, &base_instruction, counterexample)
    } else {
        base_instruction
    };

    json!({
        "status": "verification_failed",
        "error_type": failure_type,
        "location": {
            "file": span.file.clone(),
            "line": span.line
        },
        "reconstruction_loss": {
            "violated_property": atom.ensures.trim(),
            "counter_example": counter_example
        },
        "feedback_instruction": feedback_instruction
    })
}

fn build_self_correction_feedback_instruction(
    atom: &Atom,
    base_instruction: &str,
    counterexample: Option<&serde_json::Value>,
) -> String {
    let counterexample_text = counterexample
        .map(|value| value.to_string())
        .unwrap_or_else(|| "{}".to_string());
    format!(
        "Self-Correction Protocol enabled. Repair atom '{}' using the loss vector: keep the public contract intentional, change the minimal body/contract fragment needed to eliminate the counterexample {}, then rerun `mumei verify --emit loss-vector` until status is verification_passed. Violated property: `{}`. Base hint: {}",
        atom.name,
        counterexample_text,
        atom.ensures.trim(),
        base_instruction
    )
}

/// Return true when the Loss Vector represents zero reconstruction loss.
pub fn is_reconstruction_loss_empty(loss_vector: &serde_json::Value) -> bool {
    let Some(reconstruction_loss) = loss_vector.get("reconstruction_loss") else {
        return true;
    };
    if reconstruction_loss.is_null() {
        return true;
    }
    if reconstruction_loss
        .get("is_zero_loss")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return true;
    }

    reconstruction_loss
        .get("counter_example")
        .map(|counter_example| match counter_example {
            serde_json::Value::Object(entries) => entries.is_empty(),
            serde_json::Value::Array(entries) => entries.is_empty(),
            serde_json::Value::Null => true,
            _ => false,
        })
        .unwrap_or(true)
}

/// Build semantic feedback for division-by-zero violations.
pub fn build_division_by_zero_feedback(dividend_val: &str, divisor_val: &str) -> serde_json::Value {
    json!({
        "failure_type": FAILURE_DIVISION_BY_ZERO,
        "explanation": format!(
            "Division by zero: dividend = {}, divisor = {} \
             (ゼロ除算: 被除数 = {}, 除数 = {})",
            dividend_val, divisor_val, dividend_val, divisor_val
        ),
        "counter_example": {
            "dividend": dividend_val,
            "divisor": divisor_val
        },
        "suggestion": suggestion_for_failure_type(FAILURE_DIVISION_BY_ZERO)
    })
}

/// Build semantic feedback for linearity/ownership violations.
pub fn build_linearity_feedback(
    atom_name: &str,
    violations: &[String],
    span: &Span,
) -> serde_json::Value {
    let violation_details: Vec<serde_json::Value> = violations
        .iter()
        .map(|v| {
            json!({
                "description": v,
                "explanation": format!(
                    "{} (変数の線形性違反です)",
                    v
                )
            })
        })
        .collect();

    json!({
        "failure_type": FAILURE_LINEARITY_VIOLATED,
        "atom": atom_name,
        "violations": violation_details,
        "span": {
            "file": span.file,
            "line": span.line,
        },
        "suggestion": suggestion_for_failure_type(FAILURE_LINEARITY_VIOLATED)
    })
}

/// Build semantic feedback for effect containment violations.
pub fn build_effect_feedback(
    atom_name: &str,
    attempted_effect: &str,
    allowed_effects: &[String],
    missing_effects: &[String],
) -> serde_json::Value {
    json!({
        "failure_type": FAILURE_EFFECT_NOT_ALLOWED,
        "atom": atom_name,
        "attempted_effect": attempted_effect,
        "allowed_effects": allowed_effects,
        "missing_effects": missing_effects,
        "explanation": format!(
            "Effect '{}' is not allowed by the current policy. Allowed effects: {:?}. Missing: {:?} \
             (エフェクト '{}' は現在のポリシーで許可されていません。許可: {:?}、不足: {:?})",
            attempted_effect, allowed_effects, missing_effects,
            attempted_effect, allowed_effects, missing_effects
        ),
        "suggestion": suggestion_for_failure_type(FAILURE_EFFECT_NOT_ALLOWED)
    })
}

// =============================================================================
// Unsat Core: Tracking Label Parsing & Contradiction Feedback
// =============================================================================
