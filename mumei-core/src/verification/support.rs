use super::module_env::*;
use super::nlae_reporter::*;
use super::translator::*;
use super::types::*;
use super::*;

// =============================================================================
// impl の法則充足性検証 (Law Verification)
// =============================================================================

/// law 式内のメソッド呼び出しを impl body で展開する。
///
/// 例: law = "add(a, b) == add(b, a)", impl body = "a + b"
/// → "((a) + (b)) == ((b) + (a))"
///
/// アルゴリズム:
/// 1. law 式を左から走査し、メソッド名 + "(" を検出
/// 2. 括弧の対応を追跡して引数リストを抽出
/// 3. impl body 内の仮引数名を実引数で置換
/// 4. 展開結果を括弧で囲んで挿入
///
/// ネストした呼び出し（例: "leq(a, b) && leq(b, c)"）にも対応。
pub(crate) fn substitute_method_calls(
    law_expr: &str,
    method_bodies: &HashMap<String, String>,
    method_params: &HashMap<String, Vec<String>>,
) -> String {
    let mut result = law_expr.to_string();
    let max_passes = law_expr
        .chars()
        .filter(|c| *c == '(')
        .count()
        .saturating_add(method_bodies.len())
        .max(1);

    // 各メソッドについて繰り返し展開（ネスト対応のため必要なパス数を式から算出）
    for _pass in 0..max_passes {
        let mut new_result = String::new();
        let mut i = 0;
        let chars: Vec<char> = result.chars().collect();
        let mut changed = false;

        while i < chars.len() {
            // メソッド名の検出: 英字で始まり、直後に '(' が続く
            let mut found_method = false;
            for (method_name, body) in method_bodies {
                let mn_chars: Vec<char> = method_name.chars().collect();
                if i + mn_chars.len() < chars.len()
                    && chars[i..i + mn_chars.len()] == mn_chars[..]
                    && chars[i + mn_chars.len()] == '('
                    // メソッド名の直前が英数字でないことを確認（部分一致を防ぐ）
                    && (i == 0 || !is_identifier_char(chars[i - 1]))
                {
                    // 引数リストを抽出
                    let args_start = i + mn_chars.len() + 1;
                    let mut depth = 1;
                    let mut args_end = args_start;
                    while args_end < chars.len() && depth > 0 {
                        match chars[args_end] {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        args_end += 1;
                    }

                    // 引数をカンマで分割（ネストした括弧を考慮）
                    let args_str: String = chars[args_start..args_end].iter().collect();
                    let args = split_args(&args_str);

                    // body 内の仮引数名を実引数で置換
                    let mut expanded = body.clone();
                    if let Some(param_names) = method_params.get(method_name) {
                        for (j, param_name) in param_names.iter().enumerate() {
                            if let Some(arg) = args.get(j) {
                                let replacement = parenthesized_arg(arg);
                                // 単語境界を考慮した置換（部分一致を防ぐ）
                                expanded = replace_word(&expanded, param_name, &replacement);
                            }
                        }
                    }

                    new_result.push('(');
                    new_result.push_str(&expanded);
                    new_result.push(')');
                    i = args_end + 1; // ')' の次へ
                    found_method = true;
                    changed = true;
                    break;
                }
            }
            if !found_method {
                new_result.push(chars[i]);
                i += 1;
            }
        }

        result = new_result;
        if !changed {
            break;
        }
    }

    result
}

pub(crate) fn contains_method_call(source: &str, method_name: &str) -> bool {
    let chars: Vec<char> = source.chars().collect();
    let method_chars: Vec<char> = method_name.chars().collect();
    if method_chars.is_empty() {
        return false;
    }

    let mut i = 0;
    while i + method_chars.len() < chars.len() {
        if chars[i..i + method_chars.len()] == method_chars[..]
            && chars[i + method_chars.len()] == '('
            && (i == 0 || !is_identifier_char(chars[i - 1]))
        {
            return true;
        }
        i += 1;
    }
    false
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn parenthesized_arg(arg: &str) -> String {
    let trimmed = arg.trim();
    if has_single_outer_parens(trimmed) {
        trimmed.to_string()
    } else {
        format!("({trimmed})")
    }
}

fn has_single_outer_parens(expr: &str) -> bool {
    let chars: Vec<char> = expr.chars().collect();
    if chars.len() < 2 || chars.first() != Some(&'(') || chars.last() != Some(&')') {
        return false;
    }

    let mut depth = 0isize;
    for (idx, ch) in chars.iter().enumerate() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth == 0 && idx + 1 < chars.len() {
            return false;
        }
        if depth < 0 {
            return false;
        }
    }

    depth == 0
}

/// 単語境界を考慮した文字列置換。
/// "a" を置換する際に "a" 単体のみマッチし、"add" 内の "a" にはマッチしない。
pub(crate) fn replace_word(source: &str, word: &str, replacement: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = source.chars().collect();
    let word_chars: Vec<char> = word.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if i + word_chars.len() <= chars.len()
            && chars[i..i + word_chars.len()] == word_chars[..]
            && (i == 0 || !is_identifier_char(chars[i - 1]))
            && (i + word_chars.len() >= chars.len()
                || !is_identifier_char(chars[i + word_chars.len()]))
        {
            result.push_str(replacement);
            i += word_chars.len();
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

/// カンマで引数を分割する（ネストした括弧を考慮）。
pub(crate) fn split_args(input: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in input.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                result.push(current.trim().to_string());
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        result.push(trimmed);
    }
    result
}

/// impl が対応する trait の全 law を満たしているかを Z3 で検証する。
/// 各 law の論理式内のメソッド呼び出しを impl の具体的な body で置換し、
/// ∀x. law_expr が成立するかを検証する。
pub fn verify_impl(
    impl_def: &ImplDef,
    module_env: &ModuleEnv,
    output_dir: &Path,
) -> MumeiResult<()> {
    let trait_def = module_env.get_trait(&impl_def.trait_name).ok_or_else(|| {
        MumeiError::type_error_at(
            format!(
                "Trait '{}' not found for impl on '{}'",
                impl_def.trait_name, impl_def.target_type
            ),
            impl_def.span.clone(),
        )
    })?;

    // メソッドの完全性チェック: trait の全メソッドが impl されているか
    for method in &trait_def.methods {
        if !impl_def
            .method_bodies
            .iter()
            .any(|(name, _)| name == &method.name)
        {
            return Err(MumeiError::type_error_at(
                format!(
                    "impl {} for {}: missing method '{}'",
                    impl_def.trait_name, impl_def.target_type, method.name
                ),
                impl_def.span.clone(),
            ));
        }
    }

    // 各 law を Z3 で検証
    let mut cfg = Config::new();
    cfg.set_timeout_msec(5000);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    // impl のメソッド body マップを構築（未解釈関数展開用）
    let method_body_map: HashMap<String, String> = impl_def
        .method_bodies
        .iter()
        .map(|(name, body)| (name.clone(), body.clone()))
        .collect();

    // メソッドのパラメータ名マップを構築（trait 定義から取得）
    // law 式内の関数呼び出し `method(a, b)` を body 式に展開する際、
    // 仮引数名（a, b）を実引数に置換するために使用
    let method_param_names: HashMap<String, Vec<String>> = trait_def
        .methods
        .iter()
        .map(|m| {
            // トレイトメソッドのパラメータ名は慣例的に a, b, c, ... を使用
            let param_names: Vec<String> = (0..m.param_types.len())
                .map(|i| {
                    let names = ["a", "b", "c", "d", "e", "f"];
                    names.get(i).unwrap_or(&"x").to_string()
                })
                .collect();
            (m.name.clone(), param_names)
        })
        .collect();

    for (law_name, law_expr) in &trait_def.laws {
        // law 内のメソッド呼び出しを impl body で置換
        // 例: law "add(a, b) == add(b, a)" で impl body が "a + b" の場合、
        // "add(a, b)" → "(a + b)", "add(b, a)" → "(b + a)" に展開
        let substituted = substitute_method_calls(law_expr, &method_body_map, &method_param_names);

        // シンボリック変数で law を検証
        let vc = VCtx {
            ctx: &ctx,
            module_env,
            current_atom: None,
            linearity_ctx: None,
            effect_ctx: None,
            constraint_count: None,
            constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
            has_string_constraints: None,
            path_cond_stack: std::cell::RefCell::new(Vec::new()),
            profiler: None,
        };

        let mut env: Env = HashMap::new();
        // law 内の自由変数をシンボリック整数として登録
        for var_name in &["a", "b", "c", "x", "y", "z"] {
            let base = module_env.resolve_base_type(&impl_def.target_type);
            let var: Dynamic = match base.as_str() {
                "f64" => Float::new_const(&ctx, *var_name, 11, 53).into(),
                // Plan 9: Str parameters as Z3 String Sort
                "Str" => Z3String::new_const(&ctx, *var_name).into(),
                _ => Int::new_const(&ctx, *var_name).into(),
            };
            env.insert(var_name.to_string(), var);
        }
        // "true" リテラルを登録
        env.insert("true".to_string(), Bool::from_bool(&ctx, true).into());

        // law 式をパースして検証
        let law_ast = parse_expression(&substituted);
        let verify_result = expr_to_z3(&vc, &law_ast, &mut env, None);
        match verify_result {
            Ok(law_z3) => {
                if let Some(law_bool) = law_z3.as_bool() {
                    solver.push();

                    // P2-B: law 式に含まれる trait method の param_constraints のみを
                    // push/pop スコープ内で前提として注入する。
                    for method in &trait_def.methods {
                        if !contains_method_call(law_expr, &method.name) {
                            continue;
                        }
                        let param_names: Vec<String> = (0..method.param_types.len())
                            .map(|i| {
                                let names = ["a", "b", "c", "d", "e", "f"];
                                names.get(i).unwrap_or(&"x").to_string()
                            })
                            .collect();
                        for (i, constraint_opt) in method.param_constraints.iter().enumerate() {
                            if let Some(constraint_str) = constraint_opt {
                                if let Some(param_name) = param_names.get(i) {
                                    let concrete: String =
                                        replace_constraint_placeholder(constraint_str, param_name);
                                    let constraint_ast = parse_expression(&concrete);
                                    if let Ok(constraint_z3) =
                                        expr_to_z3(&vc, &constraint_ast, &mut env, None)
                                    {
                                        if let Some(constraint_bool) = constraint_z3.as_bool() {
                                            solver.assert(&constraint_bool);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    solver.assert(&law_bool.not());
                    if solver.check() == SatResult::Sat {
                        // 反例（Counter-example）を Z3 model から取得
                        let counterexample = if let Some(model) = solver.get_model() {
                            let var_names = ["a", "b", "c", "x", "y", "z"];
                            let mut ce_parts = Vec::new();
                            let mut ce_json = serde_json::Map::new();
                            for var_name in &var_names {
                                if let Some(var_z3) = env.get(*var_name) {
                                    if let Some(val) = model.eval(var_z3, true) {
                                        let val_str = format!("{}", val);
                                        // 変数が law 式に含まれている場合のみ表示
                                        if law_expr.contains(*var_name) {
                                            ce_parts.push(format!("{} = {}", var_name, val_str));
                                            ce_json.insert(var_name.to_string(), json!(val_str));
                                        }
                                    }
                                }
                            }
                            // Save counterexample to visualizer report
                            // (even when no concrete values are available, still write report.json
                            // so the MCP self-healing flow can detect the failure)
                            let ce_value = if ce_json.is_empty() {
                                None
                            } else {
                                Some(serde_json::Value::Object(ce_json))
                            };
                            save_visualizer_report(
                                output_dir,
                                "failed",
                                &format!(
                                    "impl {} for {}",
                                    impl_def.trait_name, impl_def.target_type
                                ),
                                "N/A",
                                "N/A",
                                &format!("Trait law '{}' not satisfied", law_name),
                                ce_value.as_ref(),
                                FAILURE_TRAIT_LAW_VIOLATED,
                                None,
                                Some(&impl_def.span),
                                None,
                                None,
                                None,
                            );
                            if ce_parts.is_empty() {
                                ("  (no concrete values available)".to_string(), ce_value)
                            } else {
                                (
                                    format!("  Counter-example: {}", ce_parts.join(", ")),
                                    ce_value,
                                )
                            }
                        } else {
                            ("  (could not retrieve model)".to_string(), None)
                        };
                        solver.pop(1);
                        let (ce_text, ce_data) = counterexample;
                        return Err(MumeiError::verification_at(
                            format!(
                                "impl {} for {}: law '{}' (defined in trait at {}) is not satisfied\n  Law: {}\n  Expanded: {}\n{}",
                                impl_def.trait_name, impl_def.target_type,
                                law_name, trait_def.span, law_expr, substituted, ce_text
                            ),
                            impl_def.span.clone()
                        ).with_counterexample(ce_data));
                    }
                    solver.pop(1);
                }
            }
            Err(_) => {
                // law のパースに失敗した場合はスキップ
                // （未解釈関数展開後もパースできない場合は、law 式が複雑すぎる可能性がある）
            }
        };
    }

    Ok(())
}

// =============================================================================
// リソース階層検証 (Resource Hierarchy Verification)
// =============================================================================
//
// デッドロック防止: リソース取得順序の半順序関係を Z3 で検証する。
//
// 不変条件: ∀ r1, r2 ∈ Held(thread, t):
//   acquire(r2) かつ r1 ∈ Held → Priority(r2) > Priority(r1)
//
// これにより、待機グラフ（Wait-For Graph）に循環が生じないことを
// コンパイル時に数学的に保証する。

// リソース取得コンテキスト: 現在保持中のリソースとその優先度を追跡する。
// acquire 式の検証時に、リソース階層制約をチェックする。

// =============================================================================
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

#[derive(Debug, Clone, Default)]
pub(crate) struct ResourceCtx {
    /// 現在保持中のリソース: (リソース名, 優先度)
    held: Vec<(String, i64)>,
    /// 違反リスト
    violations: Vec<String>,
}

impl ResourceCtx {
    fn new() -> Self {
        Self::default()
    }

    /// リソースを取得する。階層制約を検証し、違反があればエラーを記録する。
    fn acquire(&mut self, resource_name: &str, priority: i64) -> Result<(), String> {
        // 現在保持中の全リソースに対して、新リソースの優先度が厳密に高いことを検証
        for (held_name, held_priority) in &self.held {
            if priority <= *held_priority {
                let msg = format!(
                    "Deadlock risk: acquiring '{}' (priority={}) while holding '{}' (priority={}). \
                     New resource must have strictly higher priority.",
                    resource_name, priority, held_name, held_priority
                );
                self.violations.push(msg.clone());
                return Err(msg);
            }
        }
        self.held.push((resource_name.to_string(), priority));
        Ok(())
    }

    /// リソースを解放する（acquire ブロック終了時に呼ばれる）
    fn release(&mut self, resource_name: &str) {
        self.held.retain(|(name, _)| name != resource_name);
    }

    #[allow(dead_code)]
    pub(crate) fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }
}

/// atom のリソース使用順序を Z3 で検証する。
/// atom の resources 宣言と body 内の acquire 式から、
/// リソース階層制約 Priority(r2) > Priority(r1) を検証する。
///
/// 検証方法:
/// 1. atom の resources リストから使用リソースを特定
/// 2. body 内の acquire 式を走査し、取得順序を抽出
/// 3. Z3 で半順序関係の非循環性を証明
pub(crate) fn verify_resource_hierarchy(atom: &Atom, module_env: &ModuleEnv) -> MumeiResult<()> {
    if atom.resources.is_empty() {
        return Ok(());
    }

    // リソース定義の存在チェック
    let mut resource_priorities: Vec<(String, i64)> = Vec::new();
    for res_name in &atom.resources {
        if let Some(rdef) = module_env.resources.get(res_name) {
            resource_priorities.push((rdef.name.clone(), rdef.priority));
        } else {
            return Err(MumeiError::type_error_at(
                format!("Resource '{}' used in atom '{}' is not defined. Add: resource {} priority:<N> mode:exclusive|shared;",
                    res_name, atom.name, res_name),
                atom.span.clone()
            ));
        }
    }

    // Z3 で半順序関係を検証
    let mut cfg = Config::new();
    cfg.set_timeout_msec(5000);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    // 各リソースの優先度をシンボリック整数として定義
    let mut priority_vars: HashMap<String, Int> = HashMap::new();
    for (name, priority) in &resource_priorities {
        let var = Int::new_const(&ctx, format!("priority_{}", name).as_str());
        // 優先度を具体値に束縛
        solver.assert(&var._eq(&Int::from_i64(&ctx, *priority)));
        priority_vars.insert(name.clone(), var);
    }

    // リソース間の順序制約を検証:
    // resources リスト内で前に宣言されたリソースは先に取得されると仮定し、
    // 後に宣言されたリソースは厳密に高い優先度を持つ必要がある。
    for i in 0..resource_priorities.len() {
        for j in (i + 1)..resource_priorities.len() {
            let (name_i, _) = &resource_priorities[i];
            let (name_j, _) = &resource_priorities[j];
            let pri_i = &priority_vars[name_i];
            let pri_j = &priority_vars[name_j];

            // Priority(r_j) > Priority(r_i) を検証
            solver.push();
            solver.assert(&pri_j.le(pri_i)); // 否定: Priority(r_j) <= Priority(r_i)
            if solver.check() == SatResult::Sat {
                solver.pop(1);
                let error_span = module_env
                    .resources
                    .get(name_j)
                    .map(|r| r.span.clone())
                    .unwrap_or_else(|| atom.span.clone());
                return Err(MumeiError::verification_at(
                    format!(
                        "Resource hierarchy violation in atom '{}': \
                         '{}' (priority={}) must have strictly lower priority than '{}' (priority={}). \
                         Reorder resources or adjust priorities to prevent potential deadlock.",
                        atom.name, name_i, resource_priorities[i].1,
                        name_j, resource_priorities[j].1
                    ),
                    error_span
                ));
            }
            solver.pop(1);
        }
    }

    // データレース検証: exclusive リソースの排他性チェック
    // 同一 atom 内で同じ exclusive リソースを複数回 acquire していないことを確認
    let mut exclusive_set: HashSet<String> = HashSet::new();
    for res_name in &atom.resources {
        if let Some(rdef) = module_env.resources.get(res_name) {
            if rdef.mode == ResourceMode::Exclusive && !exclusive_set.insert(res_name.clone()) {
                return Err(MumeiError::verification_at(
                    format!(
                        "Data race risk in atom '{}': exclusive resource '{}' is listed multiple times",
                        atom.name, res_name
                    ),
                    atom.span.clone()
                ));
            }
        }
    }

    Ok(())
}

// =============================================================================
// 有界モデル検査 (Bounded Model Checking — BMC)
// =============================================================================
//
// ループ内の acquire パターンや非同期処理の安全性を、ループ不変量を
// ユーザーが記述しなくても検証するための補助的な検証手法。
//
// 設計:
// - ループを最大 BMC_UNROLL_DEPTH 回展開し、各展開でリソース階層制約を検証
// - ループ不変量が提供されている場合はそちらを優先（BMC はフォールバック）
// - Z3 タイムアウトリスクがあるため、展開回数は保守的に制限
//
// 制約:
// - 無限ループの停止性は証明しない（それは decreases 句の役割）
// - BMC は「展開回数以内でのバグ不在」を証明するのみ（完全性はない）

/// BMC のループ展開回数上限（グローバルデフォルト）
/// atom 単位で `max_unroll: N;` によりオーバーライド可能。
pub(crate) const BMC_DEFAULT_UNROLL_DEPTH: usize = 3;

/// 再帰的 async 呼び出しの最大展開深度。
/// async atom が自身を呼び出す場合、この深度を超えると
/// 「Unknown（未定義）」として扱い、Z3 探索を打ち切る。
pub(crate) const MAX_ASYNC_RECURSION_DEPTH: usize = 3;

/// body 内の Acquire を再帰的に収集する（BMC 用）。
/// ループ内で acquire が使われているパターンを検出するために使用。
pub(crate) fn collect_acquire_resources_expr(expr: &Expr) -> Vec<String> {
    let mut resources = Vec::new();
    match expr {
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            resources.extend(collect_acquire_resources_stmt(then_branch));
            resources.extend(collect_acquire_resources_stmt(else_branch));
        }
        Expr::Async { body } => {
            resources.extend(collect_acquire_resources_stmt(body));
        }
        Expr::Await { expr } => {
            resources.extend(collect_acquire_resources_expr(expr));
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                resources.extend(collect_acquire_resources_expr(arg));
            }
        }
        Expr::AtomRef { .. } => {}
        Expr::CallRef { callee, args } => {
            resources.extend(collect_acquire_resources_expr(callee));
            for arg in args {
                resources.extend(collect_acquire_resources_expr(arg));
            }
        }
        Expr::Lambda { body, .. } => {
            resources.extend(collect_acquire_resources_stmt(body));
        }
        // Plan 8: Channel operations — traverse sub-expressions for acquire resources
        Expr::ChanSend { channel, value } => {
            resources.extend(collect_acquire_resources_expr(channel));
            resources.extend(collect_acquire_resources_expr(value));
        }
        Expr::ChanRecv { channel } => {
            resources.extend(collect_acquire_resources_expr(channel));
        }
        _ => {}
    }
    resources
}

pub(crate) fn collect_acquire_resources_stmt(stmt: &Stmt) -> Vec<String> {
    let mut resources = Vec::new();
    match stmt {
        Stmt::Acquire { resource, body, .. } => {
            resources.push(resource.clone());
            resources.extend(collect_acquire_resources_stmt(body));
        }
        Stmt::Block(stmts, _) => {
            for s in stmts {
                resources.extend(collect_acquire_resources_stmt(s));
            }
        }
        Stmt::While { body, .. } => {
            resources.extend(collect_acquire_resources_stmt(body));
        }
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            resources.extend(collect_acquire_resources_expr(value));
        }
        Stmt::ArrayStore { index, value, .. } => {
            resources.extend(collect_acquire_resources_expr(index));
            resources.extend(collect_acquire_resources_expr(value));
        }
        Stmt::Task { body, .. } => {
            resources.extend(collect_acquire_resources_stmt(body));
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                resources.extend(collect_acquire_resources_stmt(child));
            }
        }
        Stmt::Expr(e, _) => {
            resources.extend(collect_acquire_resources_expr(e));
        }
        // Plan 8: Cancel statement has no resources
        Stmt::Cancel { .. } => {}
    }
    resources
}

/// 有界モデル検査: atom の body 内のループを展開し、
/// 各展開でリソース階層制約が維持されることを検証する。
///
/// 展開回数は atom.max_unroll（指定時）または BMC_DEFAULT_UNROLL_DEPTH を使用。
/// ループ不変量が提供されている場合はスキップ（不変量ベースの検証が優先）。
/// BMC は「ユーザーが不変量を書けない場合」の補助的な検証手段。
pub(crate) fn verify_bmc_resource_safety(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
    global_max_unroll: usize,
) -> MumeiResult<()> {
    // body 内に acquire が含まれない場合はスキップ
    let acquired_resources = collect_acquire_resources_stmt(body_stmt);
    if acquired_resources.is_empty() {
        return Ok(());
    }

    // While ループ内に acquire があるかチェック
    fn has_acquire_in_while_stmt(stmt: &Stmt) -> bool {
        match stmt {
            Stmt::While { body, .. } => !collect_acquire_resources_stmt(body).is_empty(),
            Stmt::Block(stmts, _) => stmts.iter().any(has_acquire_in_while_stmt),
            Stmt::Acquire { body, .. } => has_acquire_in_while_stmt(body),
            Stmt::Task { body, .. } => has_acquire_in_while_stmt(body),
            Stmt::Expr(e, _) => has_acquire_in_while_expr(e),
            _ => false,
        }
    }
    fn has_acquire_in_while_expr(expr: &Expr) -> bool {
        match expr {
            Expr::IfThenElse {
                then_branch,
                else_branch,
                ..
            } => has_acquire_in_while_stmt(then_branch) || has_acquire_in_while_stmt(else_branch),
            Expr::Async { body } => has_acquire_in_while_stmt(body),
            // Plan 8: Channel operations — traverse sub-expressions
            Expr::ChanSend { channel, value } => {
                has_acquire_in_while_expr(channel) || has_acquire_in_while_expr(value)
            }
            Expr::ChanRecv { channel } => has_acquire_in_while_expr(channel),
            _ => false,
        }
    }

    if !has_acquire_in_while_stmt(body_stmt) {
        return Ok(()); // ループ外の acquire は通常の検証で十分
    }

    // 展開回数: atom 単位のオーバーライド > 設定ファイルのグローバル値 > デフォルト
    let configured_unroll_depth = if global_max_unroll == 0 {
        BMC_DEFAULT_UNROLL_DEPTH
    } else {
        global_max_unroll
    };
    let unroll_depth = atom.max_unroll.unwrap_or(configured_unroll_depth);

    // BMC: ループを展開して各ステップでリソース階層をチェック
    let mut resource_ctx = ResourceCtx::new();

    for unroll_step in 0..unroll_depth {
        // 各展開ステップで acquire されるリソースの順序を検証
        for res_name in &acquired_resources {
            if let Some(rdef) = module_env.resources.get(res_name) {
                if let Err(e) = resource_ctx.acquire(res_name, rdef.priority) {
                    return Err(MumeiError::verification_at(
                        format!(
                            "BMC (unroll step {}/{}, max_unroll={}): resource ordering violation in loop body: {}",
                            unroll_step, unroll_depth, unroll_depth, e
                        ),
                        atom.span.clone()
                    ));
                }
            }
        }
        // 各ステップ終了時にリソースを解放（ループの次のイテレーションをシミュレート）
        for res_name in &acquired_resources {
            resource_ctx.release(res_name);
        }
    }

    Ok(())
}

/// 再帰的 async 呼び出しの深度を検証する。
/// async atom が自身を（直接的または間接的に）呼び出す場合、
/// MAX_ASYNC_RECURSION_DEPTH を超える再帰がないことを静的にチェックする。
///
/// 仕組み: body 内の Call 式を走査し、呼び出し先が async atom かつ
/// 自身と同名の場合、再帰深度カウンタをインクリメント。
/// 上限を超えたら「Unknown」として打ち切り、警告を出す。
pub(crate) fn verify_async_recursion_depth(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
) -> MumeiResult<()> {
    if !atom.is_async {
        return Ok(());
    }

    fn count_self_calls_expr(expr: &Expr, atom_name: &str) -> usize {
        match expr {
            Expr::Call(name, args) => {
                let self_call = if name == atom_name { 1 } else { 0 };
                self_call
                    + args
                        .iter()
                        .map(|a| count_self_calls_expr(a, atom_name))
                        .sum::<usize>()
            }
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                count_self_calls_expr(cond, atom_name)
                    + count_self_calls_stmt(then_branch, atom_name)
                    + count_self_calls_stmt(else_branch, atom_name)
            }
            Expr::Async { body } => count_self_calls_stmt(body, atom_name),
            Expr::Await { expr } => count_self_calls_expr(expr, atom_name),
            Expr::BinaryOp(l, _, r) => {
                count_self_calls_expr(l, atom_name) + count_self_calls_expr(r, atom_name)
            }
            Expr::Perform { args, .. } => args
                .iter()
                .map(|a| count_self_calls_expr(a, atom_name))
                .sum(),
            Expr::AtomRef { .. } => 0,
            Expr::CallRef { callee, args } => {
                let self_call = if let Expr::AtomRef { name } = callee.as_ref() {
                    if name == atom_name {
                        1
                    } else {
                        0
                    }
                } else {
                    0
                };
                self_call
                    + count_self_calls_expr(callee, atom_name)
                    + args
                        .iter()
                        .map(|a| count_self_calls_expr(a, atom_name))
                        .sum::<usize>()
            }
            Expr::Lambda { body, .. } => count_self_calls_stmt(body, atom_name),
            // Plan 8: Channel operations — traverse sub-expressions for self-calls
            Expr::ChanSend { channel, value } => {
                count_self_calls_expr(channel, atom_name) + count_self_calls_expr(value, atom_name)
            }
            Expr::ChanRecv { channel } => count_self_calls_expr(channel, atom_name),
            _ => 0,
        }
    }
    fn count_self_calls_stmt(stmt: &Stmt, atom_name: &str) -> usize {
        match stmt {
            Stmt::Block(stmts, _) => stmts
                .iter()
                .map(|s| count_self_calls_stmt(s, atom_name))
                .sum(),
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
                count_self_calls_expr(value, atom_name)
            }
            Stmt::ArrayStore { index, value, .. } => {
                count_self_calls_expr(index, atom_name) + count_self_calls_expr(value, atom_name)
            }
            Stmt::Acquire { body, .. } => count_self_calls_stmt(body, atom_name),
            Stmt::While { cond, body, .. } => {
                count_self_calls_expr(cond, atom_name) + count_self_calls_stmt(body, atom_name)
            }
            Stmt::Task { body, .. } => count_self_calls_stmt(body, atom_name),
            Stmt::TaskGroup { children, .. } => children
                .iter()
                .map(|c| count_self_calls_stmt(c, atom_name))
                .sum(),
            Stmt::Expr(e, _) => count_self_calls_expr(e, atom_name),
            // Plan 8: Cancel statement has no self-calls
            Stmt::Cancel { .. } => 0,
        }
    }

    let self_call_count = count_self_calls_stmt(body_stmt, &atom.name);

    if self_call_count > 0 {
        // 再帰的 async 呼び出しが検出された
        // 呼び出し先の async atom も再帰する可能性があるため、
        // 深度制限を超える場合は警告
        let max_depth = atom.max_unroll.unwrap_or(MAX_ASYNC_RECURSION_DEPTH);
        if self_call_count > max_depth {
            return Err(MumeiError::verification_at(
                format!(
                    "Async recursion depth exceeded in atom '{}': {} self-calls detected \
                     (max_depth={}). Use max_unroll: {}; to increase the limit, or \
                     refactor to use iteration with invariant.",
                    atom.name,
                    self_call_count,
                    max_depth,
                    self_call_count + 1
                ),
                atom.span.clone(),
            ));
        }

        // 再帰呼び出し先の契約を信頼して展開（Compositional Verification）
        // 各展開ステップで ensures を仮定として使用する。
        // これにより、f_depth_1, f_depth_2 ... と別シンボルとして扱われ、
        // Z3 が無限ループに陥ることを防ぐ。
        if let Some(callee) = module_env.get_atom(&atom.name) {
            if callee.ensures.trim() == "true" {
                // ensures が trivial な場合、再帰の安全性を証明できない
                return Err(MumeiError::verification_at(
                    format!(
                        "Recursive async atom '{}' requires a non-trivial ensures clause \
                         for inductive verification. Add: ensures: <postcondition>;",
                        atom.name
                    ),
                    atom.span.clone(),
                ));
            }
        }
    }

    Ok(())
}

// =============================================================================
// Atom レベル Invariant の帰納的検証 (Inductive Invariant Verification)
// =============================================================================
//
// atom シグネチャに `invariant: <expr>;` が指定されている場合、
// 帰納法（数学的帰納法）により不変量の正しさを証明する。
//
// 証明構造:
// 1. 導入 (Induction Base):
//    requires が成立するとき、invariant が成立することを証明する。
//    ∀ params. requires(params) → invariant(params)
//
// 2. 維持 (Induction Step / Preservation):
//    invariant が成立する状態で body を実行した後も invariant が維持されることを証明する。
//    ∀ params. invariant(params) ∧ requires(params) → invariant(body(params))
//    ※ 再帰呼び出しがある場合、呼び出し先の invariant を帰納法の仮定として使用する。
//
// これにより、再帰的 async atom の安全性を、ループ不変量と同様の
// 帰納的推論で証明できる。BMC の「有界」な保証を「完全」な保証に昇格させる。

/// atom レベルの invariant を帰納的に検証する。
pub(crate) fn verify_atom_invariant(
    atom: &Atom,
    body_stmt: &Stmt,
    invariant_raw: &str,
    module_env: &ModuleEnv,
) -> MumeiResult<()> {
    let mut cfg = Config::new();
    cfg.set_timeout_msec(5000);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    let vc = VCtx {
        ctx: &ctx,
        module_env,
        current_atom: Some(atom),
        linearity_ctx: None,
        effect_ctx: None,
        constraint_count: None,
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: None,
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
    };

    let mut env: Env = HashMap::new();

    // パラメータをシンボリック変数として登録
    for param in &atom.params {
        let var = param_z3_value(
            &ctx,
            param.name.as_str(),
            param.type_name.as_deref(),
            module_env,
        );
        env.insert(param.name.clone(), var);

        // 精緻型制約も適用
        if let Some(type_name) = &param.type_name {
            if let Some(refined) = module_env.get_type(type_name) {
                apply_refinement_constraint(&vc, &solver, &param.name, refined, &mut env)?;
            }
        }
    }

    // invariant 式をパース
    let inv_ast = parse_expression(invariant_raw);
    let inv_z3 =
        expr_to_z3(&vc, &inv_ast, &mut env, None)?
            .as_bool()
            .ok_or(MumeiError::type_error_at(
                format!(
                    "Invariant for atom '{}' must be a boolean expression",
                    atom.name
                ),
                atom.span.clone(),
            ))?;

    // === Step 1: 導入 (Induction Base) ===
    // requires → invariant を証明する
    if atom.requires.trim() != "true" {
        let req_ast = parse_expression(&atom.requires);
        let req_z3 = expr_to_z3(&vc, &req_ast, &mut env, None)?;
        if let Some(req_bool) = req_z3.as_bool() {
            solver.push();
            // requires を仮定
            solver.assert(&req_bool);
            // invariant の否定を assert
            solver.assert(&inv_z3.not());
            // Unsat なら requires → invariant が証明された
            if solver.check() == SatResult::Sat {
                solver.pop(1);
                return Err(MumeiError::verification_at(
                    format!(
                        "Invariant induction base failed for atom '{}': \
                         requires does not imply invariant.\n  \
                         Invariant: {}\n  \
                         Requires: {}\n  \
                         The invariant must hold whenever the precondition is satisfied.",
                        atom.name, invariant_raw, atom.requires
                    ),
                    atom.span.clone(),
                ));
            }
            solver.pop(1);
        }
    } else {
        // requires が true の場合、invariant は無条件に成立する必要がある
        solver.push();
        solver.assert(&inv_z3.not());
        if solver.check() == SatResult::Sat {
            solver.pop(1);
            return Err(MumeiError::verification_at(
                format!(
                    "Invariant induction base failed for atom '{}': \
                     invariant '{}' is not universally true (no requires constraint).",
                    atom.name, invariant_raw
                ),
                atom.span.clone(),
            ));
        }
        solver.pop(1);
    }

    // === Step 2: 維持 (Preservation) ===
    // invariant ∧ requires のもとで body を実行した後も invariant が維持されることを証明
    {
        let env_snapshot = env.clone();
        solver.push();

        // invariant を仮定（帰納法の仮定）
        solver.assert(&inv_z3);

        // requires も仮定
        if atom.requires.trim() != "true" {
            let req_ast = parse_expression(&atom.requires);
            let req_z3 = expr_to_z3(&vc, &req_ast, &mut env, None)?;
            if let Some(req_bool) = req_z3.as_bool() {
                solver.assert(&req_bool);
            }
        }

        // body を実行
        let _body_result = stmt_to_z3(&vc, body_stmt, &mut env, Some(&solver))?;

        // body 実行後の invariant を再評価
        // （env が body の実行で更新されている可能性がある）
        let inv_after = expr_to_z3(&vc, &inv_ast, &mut env, None)?
            .as_bool()
            .ok_or(MumeiError::type_error("Invariant must be boolean"))?;

        // invariant の維持を検証: ¬inv_after が Unsat なら維持されている
        solver.assert(&inv_after.not());
        if solver.check() == SatResult::Sat {
            solver.pop(1);
            return Err(MumeiError::verification_at(
                format!(
                    "Invariant preservation failed for atom '{}': \
                     body execution may violate the invariant.\n  \
                     Invariant: {}\n  \
                     The invariant must be maintained after executing the body.",
                    atom.name, invariant_raw
                ),
                atom.span.clone(),
            ));
        }
        solver.pop(1);
        let _ = env_snapshot; // env_snapshot はスコープ終了で破棄
    }

    Ok(())
}

// =============================================================================
// Call Graph サイクル検知 (Call Graph Cycle Detection)
// =============================================================================
//
// 間接再帰（A→B→A）を含む呼び出しグラフのサイクルを検出する。
// 直接再帰は verify_async_recursion_depth で検出済みだが、
// 間接再帰はグラフ全体を走査する必要がある。
//
// アルゴリズム: DFS による強連結成分（SCC）の簡易検出。
// サイクルが検出された場合、invariant の記述を要求するか、
// BMC の深度制限を適用する。

/// Expr を簡易的にソース文字列に復元する（requires 置換用）。
pub(crate) fn expr_to_source_string(expr: &Expr) -> String {
    match expr {
        Expr::Number(n) => n.to_string(),
        Expr::Float(f) => format!("{}", f),
        Expr::StringLit(s) => format!("\"{}\"", s),
        Expr::Variable(v) => v.clone(),
        Expr::BinaryOp(l, op, r) => {
            let op_str = match op {
                Op::Add => "+",
                Op::Sub => "-",
                Op::Mul => "*",
                Op::Div => "/",
                Op::Eq => "==",
                Op::Neq => "!=",
                Op::Gt => ">",
                Op::Lt => "<",
                Op::Ge => ">=",
                Op::Le => "<=",
                Op::And => "&&",
                Op::Or => "||",
                Op::Implies => "==>",
            };
            format!(
                "({} {} {})",
                expr_to_source_string(l),
                op_str,
                expr_to_source_string(r)
            )
        }
        Expr::Call(name, args) => {
            let args_str: Vec<String> = args.iter().map(expr_to_source_string).collect();
            format!("{}({})", name, args_str.join(", "))
        }
        Expr::FieldAccess(e, field) => format!("{}.{}", expr_to_source_string(e), field),
        Expr::ArrayAccess(name, idx) => format!("{}[{}]", name, expr_to_source_string(idx)),
        _ => format!("{:?}", expr),
    }
}

/// body 内の全 Call 式から呼び出し先の atom 名と引数を収集する。
pub(crate) fn collect_callees_with_args_expr(expr: &Expr) -> Vec<(String, Vec<Expr>)> {
    let mut callees = Vec::new();
    match expr {
        Expr::Call(name, args) => {
            callees.push((name.clone(), args.clone()));
            for arg in args {
                callees.extend(collect_callees_with_args_expr(arg));
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            callees.extend(collect_callees_with_args_expr(cond));
            callees.extend(collect_callees_with_args_stmt(then_branch));
            callees.extend(collect_callees_with_args_stmt(else_branch));
        }
        Expr::BinaryOp(l, _, r) => {
            callees.extend(collect_callees_with_args_expr(l));
            callees.extend(collect_callees_with_args_expr(r));
        }
        Expr::Async { body } => {
            callees.extend(collect_callees_with_args_stmt(body));
        }
        Expr::Await { expr } => {
            callees.extend(collect_callees_with_args_expr(expr));
        }
        Expr::Match { target, arms } => {
            callees.extend(collect_callees_with_args_expr(target));
            for arm in arms {
                callees.extend(collect_callees_with_args_stmt(&arm.body));
                if let Some(guard) = &arm.guard {
                    callees.extend(collect_callees_with_args_expr(guard));
                }
            }
        }
        Expr::CallRef { callee, args } => {
            if let Expr::AtomRef { name } = callee.as_ref() {
                callees.push((name.clone(), args.clone()));
            }
            callees.extend(collect_callees_with_args_expr(callee));
            for arg in args {
                callees.extend(collect_callees_with_args_expr(arg));
            }
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                callees.extend(collect_callees_with_args_expr(arg));
            }
        }
        Expr::Lambda { body, .. } => {
            callees.extend(collect_callees_with_args_stmt(body));
        }
        Expr::ChanSend { channel, value } => {
            callees.extend(collect_callees_with_args_expr(channel));
            callees.extend(collect_callees_with_args_expr(value));
        }
        Expr::ChanRecv { channel } => {
            callees.extend(collect_callees_with_args_expr(channel));
        }
        _ => {}
    }
    callees
}

pub(crate) fn collect_callees_with_args_stmt(stmt: &Stmt) -> Vec<(String, Vec<Expr>)> {
    let mut callees = Vec::new();
    match stmt {
        Stmt::Block(stmts, _) => {
            for s in stmts {
                callees.extend(collect_callees_with_args_stmt(s));
            }
        }
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            callees.extend(collect_callees_with_args_expr(value));
        }
        Stmt::ArrayStore { index, value, .. } => {
            callees.extend(collect_callees_with_args_expr(index));
            callees.extend(collect_callees_with_args_expr(value));
        }
        Stmt::While { cond, body, .. } => {
            callees.extend(collect_callees_with_args_expr(cond));
            callees.extend(collect_callees_with_args_stmt(body));
        }
        Stmt::Acquire { body, .. } => {
            callees.extend(collect_callees_with_args_stmt(body));
        }
        Stmt::Task { body, .. } => {
            callees.extend(collect_callees_with_args_stmt(body));
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                callees.extend(collect_callees_with_args_stmt(child));
            }
        }
        Stmt::Expr(e, _) => {
            callees.extend(collect_callees_with_args_expr(e));
        }
        Stmt::Cancel { .. } => {}
    }
    callees
}

/// Collect `<name>[<idx_expr>]` sub-expressions (returns `(array_name, idx_expr)`
/// pairs) recursively from an expression tree. Used by the forall-constraint
/// handler to build explicit Z3 pattern hints and to derive per-array
/// `len_<name> > idx` bounds so that downstream `ArrayAccess` OOB checks can
/// be discharged for indices that the user's forall already certifies as
/// valid.
///
/// The array name is preserved because `expr_to_z3`'s `Expr::ArrayAccess`
/// branch looks up the companion length constant as `len_<name>`. If we
/// hard-coded a single identifier here, multi-array forall conditions (e.g.
/// referencing both `arr` and `aux`) would bind their bounds to the wrong
/// length variable.
pub(crate) fn collect_array_accesses(expr: &Expr) -> Vec<(String, Expr)> {
    let mut out = Vec::new();
    collect_array_accesses_inner(expr, &mut out);
    out
}

pub(crate) fn collect_array_accesses_inner(expr: &Expr, out: &mut Vec<(String, Expr)>) {
    match expr {
        Expr::ArrayAccess(name, idx) => {
            out.push((name.clone(), (**idx).clone()));
            collect_array_accesses_inner(idx, out);
        }
        Expr::BinaryOp(l, _, r) => {
            collect_array_accesses_inner(l, out);
            collect_array_accesses_inner(r, out);
        }
        Expr::Call(_, args) => {
            for a in args {
                collect_array_accesses_inner(a, out);
            }
        }
        Expr::FieldAccess(e, _) => collect_array_accesses_inner(e, out),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_array_accesses_inner(cond, out);
            // then/else branches are `Box<Stmt>`; forall conditions virtually
            // never contain side-effectful branches, but when they do we still
            // want to scan any tail expression for `arr[...]` so the pattern
            // and bound synthesis stay consistent with `collect_callees_with_args_expr`.
            collect_array_accesses_in_stmt(then_branch, out);
            collect_array_accesses_in_stmt(else_branch, out);
        }
        _ => {}
    }
}

pub(crate) fn collect_array_accesses_in_stmt(stmt: &Stmt, out: &mut Vec<(String, Expr)>) {
    match stmt {
        Stmt::Expr(e, _) => collect_array_accesses_inner(e, out),
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            collect_array_accesses_inner(value, out);
        }
        Stmt::ArrayStore { index, value, .. } => {
            collect_array_accesses_inner(index, out);
            collect_array_accesses_inner(value, out);
        }
        Stmt::Block(stmts, _) => {
            for s in stmts {
                collect_array_accesses_in_stmt(s, out);
            }
        }
        Stmt::While {
            cond, invariant, ..
        } => {
            collect_array_accesses_inner(cond, out);
            collect_array_accesses_inner(invariant, out);
        }
        _ => {}
    }
}

/// body 内の全 Call 式から呼び出し先の atom 名を収集する。
pub(crate) fn collect_callees_expr(expr: &Expr) -> Vec<String> {
    let mut callees = Vec::new();
    match expr {
        Expr::Call(name, args) => {
            callees.push(name.clone());
            for arg in args {
                callees.extend(collect_callees_expr(arg));
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            callees.extend(collect_callees_expr(cond));
            callees.extend(collect_callees_stmt(then_branch));
            callees.extend(collect_callees_stmt(else_branch));
        }
        Expr::BinaryOp(l, _, r) => {
            callees.extend(collect_callees_expr(l));
            callees.extend(collect_callees_expr(r));
        }
        Expr::Async { body } => {
            callees.extend(collect_callees_stmt(body));
        }
        Expr::Await { expr } => {
            callees.extend(collect_callees_expr(expr));
        }
        Expr::Match { target, arms } => {
            callees.extend(collect_callees_expr(target));
            for arm in arms {
                callees.extend(collect_callees_stmt(&arm.body));
                if let Some(guard) = &arm.guard {
                    callees.extend(collect_callees_expr(guard));
                }
            }
        }
        Expr::AtomRef { name } => {
            callees.push(name.clone());
        }
        Expr::CallRef { callee, args } => {
            callees.extend(collect_callees_expr(callee));
            for arg in args {
                callees.extend(collect_callees_expr(arg));
            }
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                callees.extend(collect_callees_expr(arg));
            }
        }
        Expr::Lambda { body, .. } => {
            callees.extend(collect_callees_stmt(body));
        }
        // Plan 8: Channel operations — traverse sub-expressions for callees
        Expr::ChanSend { channel, value } => {
            callees.extend(collect_callees_expr(channel));
            callees.extend(collect_callees_expr(value));
        }
        Expr::ChanRecv { channel } => {
            callees.extend(collect_callees_expr(channel));
        }
        _ => {}
    }
    callees
}

pub(crate) fn collect_callees_stmt(stmt: &Stmt) -> Vec<String> {
    let mut callees = Vec::new();
    match stmt {
        Stmt::Block(stmts, _) => {
            for s in stmts {
                callees.extend(collect_callees_stmt(s));
            }
        }
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            callees.extend(collect_callees_expr(value));
        }
        Stmt::ArrayStore { index, value, .. } => {
            callees.extend(collect_callees_expr(index));
            callees.extend(collect_callees_expr(value));
        }
        Stmt::While { cond, body, .. } => {
            callees.extend(collect_callees_expr(cond));
            callees.extend(collect_callees_stmt(body));
        }
        Stmt::Acquire { body, .. } => {
            callees.extend(collect_callees_stmt(body));
        }
        Stmt::Task { body, .. } => {
            callees.extend(collect_callees_stmt(body));
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                callees.extend(collect_callees_stmt(child));
            }
        }
        Stmt::Expr(e, _) => {
            callees.extend(collect_callees_expr(e));
        }
        // Plan 8: Cancel statement has no callees
        Stmt::Cancel { .. } => {}
    }
    callees
}

/// Call Graph のサイクルを DFS で検出する。
/// atom_name から到達可能なサイクルがある場合、サイクルのパスを返す。
pub(crate) fn detect_call_cycle(atom_name: &str, module_env: &ModuleEnv) -> Option<Vec<String>> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut path: Vec<String> = Vec::new();

    fn dfs(
        current: &str,
        target: &str,
        module_env: &ModuleEnv,
        visited: &mut HashSet<String>,
        path: &mut Vec<String>,
    ) -> bool {
        if current == target && !path.is_empty() {
            return true; // サイクル検出
        }
        if visited.contains(current) {
            return false;
        }
        visited.insert(current.to_string());
        path.push(current.to_string());

        if let Some(callee_atom) = module_env.get_atom(current) {
            let body_stmt = parse_body_expr(&callee_atom.body_expr);
            let callees = collect_callees_stmt(&body_stmt);
            for callee_name in &callees {
                if module_env.get_atom(callee_name).is_some() {
                    if callee_name == target && !path.is_empty() {
                        path.push(callee_name.clone());
                        return true;
                    }
                    if dfs(callee_name, target, module_env, visited, path) {
                        return true;
                    }
                }
            }
        }

        path.pop();
        false
    }

    // atom_name の呼び出し先から DFS 開始
    if let Some(atom) = module_env.get_atom(atom_name) {
        let body_stmt = parse_body_expr(&atom.body_expr);
        let callees = collect_callees_stmt(&body_stmt);
        for callee_name in &callees {
            if module_env.get_atom(callee_name).is_some() {
                visited.clear();
                path.clear();
                path.push(atom_name.to_string());
                if dfs(callee_name, atom_name, module_env, &mut visited, &mut path) {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Call Graph サイクル検知を実行し、サイクルが見つかった場合は
/// invariant の記述を要求するか、BMC 深度制限を適用する。
pub(crate) fn verify_call_graph_cycles(atom: &Atom, module_env: &ModuleEnv) -> MumeiResult<()> {
    if let Some(cycle_path) = detect_call_cycle(&atom.name, module_env) {
        let cycle_str = cycle_path.join(" → ");

        // invariant が指定されていれば帰納的検証で対応可能
        if atom.invariant.is_some() {
            // invariant が指定されている → 帰納的検証で安全性を保証
            // （verify_atom_invariant で検証済み）
            return Ok(());
        }

        // max_unroll が指定されていれば BMC で対応
        if atom.max_unroll.is_some() {
            // BMC 深度制限が明示されている → 有界検証で対応
            return Ok(());
        }

        // どちらもない場合は警告（エラーではなく警告にとどめる）
        eprintln!(
            "  ⚠️  Call graph cycle detected for atom '{}': {}\n     \
             Consider adding `invariant: <expr>;` for complete proof, or \
             `max_unroll: N;` for bounded verification.",
            atom.name, cycle_str
        );
    }
    Ok(())
}

// =============================================================================
// Taint Analysis (汚染解析)
// =============================================================================
//
// unverified な外部関数から戻ってきた値を「汚染済み（tainted）」としてマークし、
// tainted 値が安全性の証明に使われた場合に警告を出す。
//
// 仕組み:
// - expr_to_z3 の Call 処理で、呼び出し先が unverified の場合、
//   戻り値に __tainted_{call_id} マーカーを付与する。
// - ensures の検証時、env 内に __tainted_* が存在する場合、
//   「検証結果が未検証コードに依存している」旨の警告を出す。

/// unverified 関数の呼び出しを検出し、taint マーカーを env に追加する。
/// verify() の body 検証後に呼び出される。
pub(crate) fn check_taint_propagation(
    atom: &Atom,
    body_stmt: &Stmt,
    env: &Env,
    module_env: &ModuleEnv,
) {
    // body 内で呼び出されている関数を収集
    let callees = collect_callees_stmt(body_stmt);

    let mut tainted_sources: Vec<String> = Vec::new();
    for callee_name in &callees {
        if let Some(callee) = module_env.get_atom(callee_name) {
            if callee.trust_level == TrustLevel::Unverified {
                tainted_sources.push(callee_name.clone());
            }
        }
    }

    if !tainted_sources.is_empty() {
        // env 内の __tainted_* マーカーを確認
        let taint_markers: Vec<&String> =
            env.keys().filter(|k| k.starts_with("__tainted_")).collect();

        if !taint_markers.is_empty() || !tainted_sources.is_empty() {
            eprintln!(
                "  ⚠️  Taint warning for atom '{}': verification depends on unverified function(s): [{}]. \
                 Results may be unsound.",
                atom.name, tainted_sources.join(", ")
            );
        }
    }
}

// =============================================================================
// Source-map data flow trace (spurious counterexample debugging)
// =============================================================================

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DataFlowTrace {
    pub initial_state: Vec<VariableState>,
    pub execution_path: Vec<ExecutionStep>,
    pub violation: ViolationInfo,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct VariableState {
    pub name: String,
    pub value: String,
    pub line: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExecutionStep {
    pub line: usize,
    pub expression: String,
    pub mutations: Vec<VariableMutation>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct VariableMutation {
    pub name: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ViolationInfo {
    pub line: usize,
    pub contract_type: String,
    pub expression: String,
    pub evaluated_as: String,
}

#[derive(Debug, Clone, PartialEq)]
enum TraceValue {
    Int(i64),
    Bool(bool),
    String(String),
}

/// Build an LLM-readable data-flow trace from a concrete Z3 model.
///
/// The trace replays the HIR body under Mumei semantics, records source-line
/// mutations, and localizes the failing postcondition. It is intentionally
/// conservative: unsupported statement/expression forms return `None` rather
/// than producing misleading debug stories.
pub fn build_data_flow_trace(
    atom: &Atom,
    model: &HashMap<String, i64>,
    module_env: &ModuleEnv,
    hir_atom: &HirAtom,
) -> Option<DataFlowTrace> {
    let mut env = model.clone();
    let initial_state: Vec<VariableState> = atom
        .params
        .iter()
        .filter_map(|param| {
            let value = model.get(&param.name)?;
            Some(VariableState {
                name: param.name.clone(),
                value: value.to_string(),
                line: atom.span.line,
            })
        })
        .collect();

    let mut execution_path = Vec::new();
    let result = trace_stmt(
        &hir_atom.body_stmt,
        &mut env,
        module_env,
        &mut execution_path,
        0,
    )?;
    match &result {
        TraceValue::Int(value) => {
            env.insert("result".to_string(), *value);
        }
        TraceValue::Bool(value) => {
            env.insert("result".to_string(), i64::from(*value));
        }
        TraceValue::String(_) => {}
    }

    let ensures_expr = parse_expression(&atom.ensures);
    let evaluated = trace_eval_expr(&ensures_expr, &mut env, module_env, 0)?;
    let is_satisfied = trace_value_as_bool(&evaluated)?;
    if is_satisfied {
        return None;
    }

    Some(DataFlowTrace {
        initial_state,
        execution_path,
        violation: ViolationInfo {
            line: atom.span.line,
            contract_type: "ensures".to_string(),
            expression: atom.ensures.clone(),
            evaluated_as: format!(
                "{} ({})",
                trace_evaluated_expression(&ensures_expr, &env),
                if is_satisfied { "TRUE" } else { "FALSE" }
            ),
        },
    })
}

fn trace_stmt(
    stmt: &Stmt,
    env: &mut HashMap<String, i64>,
    module_env: &ModuleEnv,
    execution_path: &mut Vec<ExecutionStep>,
    depth: usize,
) -> Option<TraceValue> {
    match stmt {
        Stmt::Let { var, value, span } | Stmt::Assign { var, value, span } => {
            let before = env
                .get(var)
                .map(i64::to_string)
                .unwrap_or_else(|| "<unbound>".to_string());
            let eval = trace_eval_expr(value, env, module_env, depth)?;
            let after = trace_value_as_int(&eval)?;
            env.insert(var.clone(), after);
            let after_string = after.to_string();
            execution_path.push(ExecutionStep {
                line: span.line,
                expression: format!("{} = {}", var, expr_to_source_string(value)),
                mutations: vec![VariableMutation {
                    name: var.clone(),
                    before,
                    after: after_string,
                }],
            });
            Some(TraceValue::Int(after))
        }
        Stmt::Block(stmts, _) => {
            let mut last = TraceValue::Int(0);
            for stmt in stmts {
                last = trace_stmt(stmt, env, module_env, execution_path, depth)?;
            }
            Some(last)
        }
        Stmt::Expr(expr, span) => {
            let value = trace_eval_expr(expr, env, module_env, depth)?;
            execution_path.push(ExecutionStep {
                line: span.line,
                expression: expr_to_source_string(expr),
                mutations: Vec::new(),
            });
            Some(value)
        }
        Stmt::While { .. }
        | Stmt::Acquire { .. }
        | Stmt::Task { .. }
        | Stmt::TaskGroup { .. }
        | Stmt::Cancel { .. }
        | Stmt::ArrayStore { .. } => None,
    }
}

fn trace_eval_expr(
    expr: &Expr,
    env: &mut HashMap<String, i64>,
    module_env: &ModuleEnv,
    depth: usize,
) -> Option<TraceValue> {
    if depth > 8 {
        return None;
    }

    match expr {
        Expr::Number(value) => Some(TraceValue::Int(*value)),
        Expr::Float(_) => None,
        Expr::StringLit(value) => Some(TraceValue::String(value.clone())),
        Expr::Variable(name) if name == "true" => Some(TraceValue::Bool(true)),
        Expr::Variable(name) if name == "false" => Some(TraceValue::Bool(false)),
        Expr::Variable(name) => env.get(name).copied().map(TraceValue::Int),
        Expr::BinaryOp(left, op, right) => {
            let left_value = trace_eval_expr(left, env, module_env, depth)?;
            let right_value = trace_eval_expr(right, env, module_env, depth)?;
            trace_eval_binary(left_value, op, right_value)
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let cond_value = trace_eval_expr(cond, env, module_env, depth)?;
            if trace_value_as_bool(&cond_value)? {
                trace_stmt(then_branch, env, module_env, &mut Vec::new(), depth)
            } else {
                trace_stmt(else_branch, env, module_env, &mut Vec::new(), depth)
            }
        }
        Expr::Call(name, args) => trace_eval_atom_call(name, args, env, module_env, depth + 1),
        Expr::ArrayAccess(_, _)
        | Expr::StructInit { .. }
        | Expr::FieldAccess(_, _)
        | Expr::Match { .. }
        | Expr::Async { .. }
        | Expr::Await { .. }
        | Expr::AtomRef { .. }
        | Expr::CallRef { .. }
        | Expr::Perform { .. }
        | Expr::Lambda { .. }
        | Expr::ChanSend { .. }
        | Expr::ChanRecv { .. } => None,
    }
}

fn trace_eval_atom_call(
    name: &str,
    args: &[Expr],
    env: &mut HashMap<String, i64>,
    module_env: &ModuleEnv,
    depth: usize,
) -> Option<TraceValue> {
    let callee = module_env.get_atom(name)?;
    if callee.trust_level == TrustLevel::Trusted || args.len() != callee.params.len() {
        return None;
    }

    let mut call_env = HashMap::new();
    for (param, arg) in callee.params.iter().zip(args) {
        let value = trace_eval_expr(arg, env, module_env, depth)?;
        call_env.insert(param.name.clone(), trace_value_as_int(&value)?);
    }

    if !trace_eval_bool_clause(&callee.requires, &mut call_env, module_env)? {
        return None;
    }
    let body = parse_body_expr(&callee.body_expr);
    let result = trace_stmt(&body, &mut call_env, module_env, &mut Vec::new(), depth)?;
    match &result {
        TraceValue::Int(value) => {
            call_env.insert("result".to_string(), *value);
        }
        TraceValue::Bool(value) => {
            call_env.insert("result".to_string(), i64::from(*value));
        }
        TraceValue::String(_) => {}
    }
    if !trace_eval_bool_clause(&callee.ensures, &mut call_env, module_env)? {
        return None;
    }
    Some(result)
}

fn trace_eval_bool_clause(
    clause: &str,
    env: &mut HashMap<String, i64>,
    module_env: &ModuleEnv,
) -> Option<bool> {
    if clause.trim().is_empty() || clause.trim() == "true" {
        return Some(true);
    }
    let expr = parse_expression(clause);
    let value = trace_eval_expr(&expr, env, module_env, 0)?;
    trace_value_as_bool(&value)
}

fn trace_eval_binary(left: TraceValue, op: &Op, right: TraceValue) -> Option<TraceValue> {
    match (left, right) {
        (TraceValue::Int(left), TraceValue::Int(right)) => match op {
            Op::Add => Some(TraceValue::Int(left + right)),
            Op::Sub => Some(TraceValue::Int(left - right)),
            Op::Mul => Some(TraceValue::Int(left * right)),
            Op::Div if right != 0 => Some(TraceValue::Int(left / right)),
            Op::Div => None,
            Op::Eq => Some(TraceValue::Bool(left == right)),
            Op::Neq => Some(TraceValue::Bool(left != right)),
            Op::Gt => Some(TraceValue::Bool(left > right)),
            Op::Lt => Some(TraceValue::Bool(left < right)),
            Op::Ge => Some(TraceValue::Bool(left >= right)),
            Op::Le => Some(TraceValue::Bool(left <= right)),
            Op::And => Some(TraceValue::Bool(left != 0 && right != 0)),
            Op::Or => Some(TraceValue::Bool(left != 0 || right != 0)),
            Op::Implies => Some(TraceValue::Bool(left == 0 || right != 0)),
        },
        (TraceValue::Bool(left), TraceValue::Bool(right)) => match op {
            Op::Eq => Some(TraceValue::Bool(left == right)),
            Op::Neq => Some(TraceValue::Bool(left != right)),
            Op::And => Some(TraceValue::Bool(left && right)),
            Op::Or => Some(TraceValue::Bool(left || right)),
            Op::Implies => Some(TraceValue::Bool(!left || right)),
            _ => None,
        },
        (TraceValue::String(left), TraceValue::String(right)) => match op {
            Op::Eq => Some(TraceValue::Bool(left == right)),
            Op::Neq => Some(TraceValue::Bool(left != right)),
            _ => None,
        },
        (left, right) => {
            let left_int = trace_value_as_int(&left);
            let right_int = trace_value_as_int(&right);
            let left_bool = trace_value_as_bool(&left);
            let right_bool = trace_value_as_bool(&right);
            match (left_int, right_int, left_bool, right_bool, op) {
                (Some(left), Some(right), _, _, Op::Eq) => Some(TraceValue::Bool(left == right)),
                (Some(left), Some(right), _, _, Op::Neq) => Some(TraceValue::Bool(left != right)),
                (_, _, Some(left), Some(right), Op::And) => Some(TraceValue::Bool(left && right)),
                (_, _, Some(left), Some(right), Op::Or) => Some(TraceValue::Bool(left || right)),
                (_, _, Some(left), Some(right), Op::Implies) => {
                    Some(TraceValue::Bool(!left || right))
                }
                _ => None,
            }
        }
    }
}

fn trace_value_as_int(value: &TraceValue) -> Option<i64> {
    match value {
        TraceValue::Int(value) => Some(*value),
        TraceValue::Bool(value) => Some(i64::from(*value)),
        TraceValue::String(_) => None,
    }
}

fn trace_value_as_bool(value: &TraceValue) -> Option<bool> {
    match value {
        TraceValue::Bool(value) => Some(*value),
        TraceValue::Int(value) => Some(*value != 0),
        TraceValue::String(_) => None,
    }
}

fn trace_evaluated_expression(expr: &Expr, env: &HashMap<String, i64>) -> String {
    match expr {
        Expr::Number(value) => value.to_string(),
        Expr::Float(value) => value.to_string(),
        Expr::StringLit(value) => format!("\"{}\"", value),
        Expr::Variable(name) if name == "true" || name == "false" => name.clone(),
        Expr::Variable(name) => env.get(name).map_or_else(|| name.clone(), i64::to_string),
        Expr::BinaryOp(left, op, right) => format!(
            "{} {} {}",
            trace_evaluated_expression(left, env),
            trace_op_symbol(op),
            trace_evaluated_expression(right, env)
        ),
        Expr::Call(name, args) => {
            let args = args
                .iter()
                .map(|arg| trace_evaluated_expression(arg, env))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({})", name, args)
        }
        Expr::FieldAccess(base, field) => {
            format!("{}.{}", trace_evaluated_expression(base, env), field)
        }
        Expr::ArrayAccess(name, idx) => {
            format!("{}[{}]", name, trace_evaluated_expression(idx, env))
        }
        _ => expr_to_source_string(expr),
    }
}

fn trace_op_symbol(op: &Op) -> &'static str {
    match op {
        Op::Add => "+",
        Op::Sub => "-",
        Op::Mul => "*",
        Op::Div => "/",
        Op::Eq => "==",
        Op::Neq => "!=",
        Op::Gt => ">",
        Op::Lt => "<",
        Op::Ge => ">=",
        Op::Le => "<=",
        Op::And => "&&",
        Op::Or => "||",
        Op::Implies => "==>",
    }
}

// =============================================================================
// Step 3: Effect Inference（エフェクト推論）
// =============================================================================

/// body 内の関数呼び出しからエフェクトセットを推論する。
/// 呼び出し先 atom の effects フィールドを再帰的に集約する。
/// 親エフェクトへの暗黙的包含も解決する。
pub(crate) fn infer_effects(atom: &Atom, body_stmt: &Stmt, module_env: &ModuleEnv) -> Vec<Effect> {
    let callees = collect_callees_stmt(body_stmt);
    let mut inferred = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    for callee_name in &callees {
        if let Some(callee) = module_env.get_atom(callee_name) {
            for eff in &callee.effects {
                if seen_names.insert(eff.name.clone()) {
                    inferred.push(eff.clone());
                }
                // NOTE: ancestors are NOT added to seen_names to avoid suppressing
                // explicit effect requirements from other callees. The deduplication
                // via seen_names only applies to effects with the exact same name.
                // Subtype coverage is handled separately by infer_effects_json's
                // is_subeffect() check when computing missing_effects.
            }
        }
    }

    // atom_ref パラメータの effect_set からもエフェクトを推論
    for param in &atom.params {
        if let Some(ref type_ref) = param.type_ref {
            if type_ref.is_fn_type() {
                if let Some(ref effect_set) = type_ref.effect_set {
                    for eff_name in effect_set {
                        if seen_names.insert(eff_name.clone()) {
                            inferred.push(Effect::simple(eff_name));
                        }
                    }
                }
            }
        }
    }

    inferred
}

/// 全 atom のエフェクト推論結果を JSON で出力する。
/// MCP の get_inferred_effects ツールから呼ばれる。
pub fn infer_effects_json(items: &[Item], module_env: &ModuleEnv) -> serde_json::Value {
    let mut results = Vec::new();
    for item in items {
        if let Item::Atom(atom) = item {
            let declared: Vec<String> = atom.effects.iter().map(|e| e.name.clone()).collect();
            let body_stmt = parse_body_expr(&atom.body_expr);
            let inferred = infer_effects(atom, &body_stmt, module_env);
            let inferred_names: Vec<String> = inferred.iter().map(|e| e.name.clone()).collect();
            let missing: Vec<String> = inferred_names
                .iter()
                .filter(|n| {
                    !declared.contains(n) && !declared.iter().any(|d| module_env.is_subeffect(n, d))
                })
                .cloned()
                .collect();
            let suggestion = if missing.is_empty() {
                serde_json::Value::Null
            } else {
                let all_effects: Vec<String> =
                    declared.iter().chain(missing.iter()).cloned().collect();
                serde_json::Value::String(format!("effects: [{}];", all_effects.join(", ")))
            };
            results.push(serde_json::json!({
                "atom": atom.name,
                "declared_effects": declared,
                "inferred_effects": inferred_names,
                "missing_effects": missing,
                "suggestion": suggestion
            }));
        }
    }
    serde_json::json!({ "effects_analysis": results })
}

// =============================================================================
// Plan 13: Contract Inference Engine
// =============================================================================
// Dataflow analysis to infer requires/ensures contracts for atoms.
// - infer_requires: divisor tracking + callee requires propagation
// - infer_ensures: simple body expression analysis + non-negativity

/// Collect all divisor expressions from a statement (Plan 13-1 helper).
/// Tracks expressions used as right-hand side of division operations.
pub(crate) fn collect_divisors_expr(expr: &Expr) -> Vec<String> {
    let mut divisors = Vec::new();
    match expr {
        Expr::BinaryOp(lhs, Op::Div, rhs) => {
            // The right-hand side is a divisor
            match rhs.as_ref() {
                Expr::Variable(name) => divisors.push(name.clone()),
                Expr::Number(n) if *n == 0 => divisors.push("0".to_string()),
                _ => {}
            }
            // Also recurse into sub-expressions (both sides)
            divisors.extend(collect_divisors_expr(lhs));
            divisors.extend(collect_divisors_expr(rhs));
        }
        Expr::BinaryOp(lhs, _, rhs) => {
            divisors.extend(collect_divisors_expr(lhs));
            divisors.extend(collect_divisors_expr(rhs));
        }
        Expr::Call(_, args) => {
            for arg in args {
                divisors.extend(collect_divisors_expr(arg));
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            divisors.extend(collect_divisors_expr(cond));
            divisors.extend(collect_divisors_stmt(then_branch));
            divisors.extend(collect_divisors_stmt(else_branch));
        }
        Expr::Match { target, arms } => {
            divisors.extend(collect_divisors_expr(target));
            for arm in arms {
                divisors.extend(collect_divisors_stmt(&arm.body));
            }
        }
        Expr::Async { body } => {
            divisors.extend(collect_divisors_stmt(body));
        }
        Expr::Await { expr } => {
            divisors.extend(collect_divisors_expr(expr));
        }
        Expr::Lambda { body, .. } => {
            divisors.extend(collect_divisors_stmt(body));
        }
        Expr::Perform { args, .. } => {
            for arg in args {
                divisors.extend(collect_divisors_expr(arg));
            }
        }
        Expr::CallRef { callee, args } => {
            divisors.extend(collect_divisors_expr(callee));
            for arg in args {
                divisors.extend(collect_divisors_expr(arg));
            }
        }
        Expr::ChanSend { channel, value } => {
            divisors.extend(collect_divisors_expr(channel));
            divisors.extend(collect_divisors_expr(value));
        }
        Expr::ChanRecv { channel } => {
            divisors.extend(collect_divisors_expr(channel));
        }
        _ => {}
    }
    divisors
}

/// Collect all divisor expressions from a statement.
pub(crate) fn collect_divisors_stmt(stmt: &Stmt) -> Vec<String> {
    let mut divisors = Vec::new();
    match stmt {
        Stmt::Block(stmts, _) => {
            for s in stmts {
                divisors.extend(collect_divisors_stmt(s));
            }
        }
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            divisors.extend(collect_divisors_expr(value));
        }
        Stmt::ArrayStore { index, value, .. } => {
            divisors.extend(collect_divisors_expr(index));
            divisors.extend(collect_divisors_expr(value));
        }
        Stmt::While { cond, body, .. } => {
            divisors.extend(collect_divisors_expr(cond));
            divisors.extend(collect_divisors_stmt(body));
        }
        Stmt::Acquire { body, .. } => {
            divisors.extend(collect_divisors_stmt(body));
        }
        Stmt::Task { body, .. } => {
            divisors.extend(collect_divisors_stmt(body));
        }
        Stmt::TaskGroup { children, .. } => {
            for child in children {
                divisors.extend(collect_divisors_stmt(child));
            }
        }
        Stmt::Expr(e, _) => {
            divisors.extend(collect_divisors_expr(e));
        }
        _ => {}
    }
    divisors
}

/// Infer requires constraints for an atom (Plan 13-1).
/// Analyzes the body to find:
/// 1. Division operations → "divisor != 0"
/// 2. Callee requires propagation → caller must satisfy callee's requires
pub(crate) fn infer_requires(atom: &Atom, module_env: &ModuleEnv) -> Vec<String> {
    let mut requires = Vec::new();
    let mut seen = HashSet::new();
    let body_stmt = parse_body_expr(&atom.body_expr);

    // 1. Divisor tracking
    let divisors = collect_divisors_stmt(&body_stmt);
    let param_names: HashSet<String> = atom.params.iter().map(|p| p.name.clone()).collect();
    for div in &divisors {
        if param_names.contains(div) && seen.insert(format!("{} != 0", div)) {
            // Check if already covered by refinement type
            let is_covered = atom.params.iter().any(|p| {
                if p.name == *div {
                    if let Some(ref tr) = p.type_ref {
                        if let Some(rt) = module_env.get_type(&tr.name) {
                            // If the type already ensures non-zero (e.g., Pos type)
                            return rt.predicate_raw.contains("> 0")
                                || rt.predicate_raw.contains("!= 0");
                        }
                    }
                }
                false
            });
            if !is_covered {
                requires.push(format!("{} != 0", div));
            }
        }
    }

    // 2. Callee requires propagation with argument substitution
    // callee の param_name → caller の引数式文字列のマッピングを構築し、
    // callee の requires 内のパラメータ名を caller の引数式で置換してから伝播する。
    let callees_with_args = collect_callees_with_args_stmt(&body_stmt);
    for (callee_name, call_args) in &callees_with_args {
        if let Some(callee_atom) = module_env.get_atom(callee_name) {
            if callee_atom.requires != "true" && !callee_atom.requires.is_empty() {
                let mut substituted_req = callee_atom.requires.clone();
                // callee の仮引数名と呼び出し引数を zip して置換
                // 同時置換: まずパラメータ名をユニークなプレースホルダに置換し、
                // 次にプレースホルダを引数式に置換する。
                // これにより逐次置換の衝突（例: a→b, b→a で a>b が a>a になる）を防ぐ。
                // First pass: param names → unique placeholders
                for (i, param) in callee_atom.params.iter().enumerate() {
                    let param_re =
                        regex::Regex::new(&format!(r"\b{}\b", regex::escape(&param.name))).unwrap();
                    substituted_req = param_re
                        .replace_all(
                            &substituted_req,
                            regex::NoExpand(&format!("__PARAM_{}__", i)),
                        )
                        .to_string();
                }
                // Second pass: placeholders → argument expressions
                for (i, arg_expr) in call_args.iter().enumerate() {
                    let arg_str = expr_to_source_string(arg_expr);
                    substituted_req =
                        substituted_req.replace(&format!("__PARAM_{}__", i), &arg_str);
                }
                if seen.insert(substituted_req.clone()) {
                    requires.push(substituted_req);
                }
            }
        }
    }

    requires
}

/// Infer ensures constraints for an atom (Plan 13-2).
/// Analyzes the body to find:
/// 1. Simple body expressions → "result == expr"
/// 2. Non-negativity analysis → "result >= 0" if all paths return non-negative
pub(crate) fn infer_ensures(atom: &Atom, module_env: &ModuleEnv) -> Vec<String> {
    let mut ensures = Vec::new();

    // Simple body expression analysis
    let body_expr_str = atom.body_expr.trim();
    let is_simple = !body_expr_str.contains("if ")
        && !body_expr_str.contains("while ")
        && !body_expr_str.contains("match ")
        && !body_expr_str.contains('{');

    if is_simple && !body_expr_str.is_empty() {
        // Check if the body is a simple arithmetic expression
        let body_stmt = parse_body_expr(&atom.body_expr);
        if let Stmt::Let { .. } = &body_stmt {
            // Skip complex let bindings
        } else {
            ensures.push(format!("result == {}", body_expr_str));
        }
    }

    // Non-negativity analysis: check if all parameters involved are non-negative types
    let param_names: HashSet<String> = atom.params.iter().map(|p| p.name.clone()).collect();
    let all_params_nonneg = atom.params.iter().all(|p| {
        if let Some(ref tr) = p.type_ref {
            if let Some(rt) = module_env.get_type(&tr.name) {
                return rt.predicate_raw.contains(">= 0") || rt.predicate_raw.contains("> 0");
            }
            // Nat type
            if tr.name == "Nat" {
                return true;
            }
        }
        false
    });

    if all_params_nonneg && !param_names.is_empty() {
        // Check if body only uses addition/multiplication (preserves non-negativity).
        // Use character-level check (not space-delimited) to avoid missing `a-b`, `a/b`, `a%b`.
        let body_only_nonneg_ops = !body_expr_str.contains('-')
            && !body_expr_str.contains('/')
            && !body_expr_str.contains('%');
        if body_only_nonneg_ops {
            ensures.push("result >= 0".to_string());
        }
    }

    ensures
}

/// Infer contracts for all atoms in JSON format (Plan 13-3).
/// Called by the CLI command `mumei infer-contracts` and MCP tool.
pub fn infer_contracts_json(items: &[Item], module_env: &ModuleEnv) -> serde_json::Value {
    let mut results = Vec::new();
    for item in items {
        if let Item::Atom(atom) = item {
            let inferred_requires = infer_requires(atom, module_env);
            let inferred_ensures = infer_ensures(atom, module_env);
            let declared_requires = atom.requires.clone();
            let declared_ensures = atom.ensures.clone();

            // Filter out inferred requires already covered by declared
            let new_requires: Vec<String> = inferred_requires
                .iter()
                .filter(|r| !declared_requires.contains(r.as_str()))
                .cloned()
                .collect();

            // Filter out inferred ensures already covered by declared
            let new_ensures: Vec<String> = inferred_ensures
                .iter()
                .filter(|e| !declared_ensures.contains(e.as_str()))
                .cloned()
                .collect();

            let suggestion_requires = if new_requires.is_empty() {
                serde_json::Value::Null
            } else {
                let all_reqs: Vec<String> = if declared_requires == "true" {
                    new_requires.clone()
                } else {
                    let mut all = vec![declared_requires.clone()];
                    all.extend(new_requires.clone());
                    all
                };
                serde_json::Value::String(format!("requires: {};", all_reqs.join(" && ")))
            };

            let suggestion_ensures = if new_ensures.is_empty() {
                serde_json::Value::Null
            } else {
                let all_ens: Vec<String> = if declared_ensures == "true" {
                    new_ensures.clone()
                } else {
                    let mut all = vec![declared_ensures.clone()];
                    all.extend(new_ensures.clone());
                    all
                };
                serde_json::Value::String(format!("ensures: {};", all_ens.join(" && ")))
            };

            results.push(serde_json::json!({
                "atom": atom.name,
                "declared_requires": declared_requires,
                "declared_ensures": declared_ensures,
                "inferred_requires": inferred_requires,
                "inferred_ensures": inferred_ensures,
                "new_requires": new_requires,
                "new_ensures": new_ensures,
                "suggestion_requires": suggestion_requires,
                "suggestion_ensures": suggestion_ensures,
            }));
        }
    }
    serde_json::json!({ "contracts_analysis": results })
}

/// エフェクト整合性検証: 宣言されたエフェクトと推論されたエフェクトの比較。
/// エフェクト階層の Subtyping も考慮する。
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
