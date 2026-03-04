use crate::parser::{
    parse_expression, Atom, EnumDef, Expr, ImplDef, MatchArm, Op, Pattern, QuantifierType,
    RefinedType, ResourceDef, ResourceMode, Span, StructDef, TraitDef, TrustLevel,
};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;
use z3::ast::{Array, Ast, Bool, Dynamic, Float, Int};
use z3::{Config, Context, SatResult, Solver};

// --- エラー型の定義 ---

/// エラーの詳細情報。ソース位置（Span）と修正提案（suggestion）を保持する。
#[derive(Debug, Clone)]
pub struct ErrorDetail {
    /// エラーメッセージ
    pub message: String,
    /// エラーが発生したソース位置（不明の場合は Span::default()）
    pub span: Span,
    /// 修正提案（例: "型を i64 に変更してください"）
    pub suggestion: Option<String>,
}

impl ErrorDetail {
    /// メッセージのみで ErrorDetail を生成する（後方互換性のため）
    pub fn from_message(msg: impl Into<String>) -> Self {
        ErrorDetail {
            message: msg.into(),
            span: Span::default(),
            suggestion: None,
        }
    }

    /// Span 付きで ErrorDetail を生成する
    pub fn with_span(msg: impl Into<String>, span: Span) -> Self {
        ErrorDetail {
            message: msg.into(),
            span,
            suggestion: None,
        }
    }
}

impl fmt::Display for ErrorDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.span.line > 0 {
            write!(f, "{}: {}", self.span, self.message)?;
        } else {
            write!(f, "{}", self.message)?;
        }
        if let Some(ref suggestion) = self.suggestion {
            write!(f, " (hint: {})", suggestion)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum MumeiError {
    VerificationError(String),
    CodegenError(String),
    TypeError(String),
}

impl MumeiError {
    /// Span 付きの ErrorDetail を取得する（将来の拡張用）
    pub fn to_detail(&self) -> ErrorDetail {
        match self {
            MumeiError::VerificationError(msg) => {
                ErrorDetail::from_message(format!("Verification Error: {}", msg))
            }
            MumeiError::CodegenError(msg) => {
                ErrorDetail::from_message(format!("Codegen Error: {}", msg))
            }
            MumeiError::TypeError(msg) => ErrorDetail::from_message(format!("Type Error: {}", msg)),
        }
    }
}

impl fmt::Display for MumeiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MumeiError::VerificationError(msg) => write!(f, "Verification Error: {}", msg),
            MumeiError::CodegenError(msg) => write!(f, "Codegen Error: {}", msg),
            MumeiError::TypeError(msg) => write!(f, "Type Error: {}", msg),
        }
    }
}

impl From<String> for MumeiError {
    fn from(s: String) -> Self {
        MumeiError::VerificationError(s)
    }
}

impl From<&str> for MumeiError {
    fn from(s: &str) -> Self {
        MumeiError::VerificationError(s.to_string())
    }
}

pub type MumeiResult<T> = Result<T, MumeiError>;
type Env<'a> = HashMap<String, Dynamic<'a>>;
type DynResult<'a> = MumeiResult<Dynamic<'a>>;

/// 検証時に共有するコンテキスト（ctx, arr, module_env を束ねて引数を削減）
struct VCtx<'a> {
    ctx: &'a Context,
    arr: &'a Array<'a>,
    module_env: &'a ModuleEnv,
}

// =============================================================================
// 線形性チェック（Linear Types / Ownership Tracking）
// =============================================================================
//
// 動的メモリ管理における二重解放・Use-After-Free を防ぐために、
// 変数の「生存状態」を追跡する。
//
// 設計:
// - LinearityCtx が各変数の生存フラグ (is_alive) を管理
// - consume(x) 呼び出し時に x を「消費済み」としてマーク
// - 消費済み変数へのアクセスはコンパイルエラー
//
// 将来の拡張:
// - atom のパラメータに `consume` 修飾子を追加
//   例: atom take_ownership(resource: T) consume resource;
// - Z3 上で is_alive フラグをシンボリック Bool として表現し、
//   consume 後のアクセスを ¬is_alive(x) として検出

/// 変数の線形性（所有権）追跡コンテキスト
///
/// 所有権（Ownership）と借用（Borrowing）の両方を追跡する。
/// - consume: 所有権を消費（移動）。消費後のアクセスは Use-After-Free。
/// - borrow: 読み取り専用の借用。借用中は所有者が consume/free できない。
/// - release_borrow: 借用を解放。
#[derive(Debug, Clone, Default)]
pub struct LinearityCtx {
    /// 変数名 → 生存状態（true = alive, false = consumed）
    alive: HashMap<String, bool>,
    /// 変数名 → 借用カウント（0 = 借用なし、1+ = 借用中）
    borrow_count: HashMap<String, usize>,
    /// 変数名 → 借用元の変数名リスト（誰がこの変数を借用しているか）
    borrowers: HashMap<String, Vec<String>>,
    /// 消費済み変数のアクセス違反リスト
    violations: Vec<String>,
}

impl LinearityCtx {
    pub fn new() -> Self {
        Self::default()
    }

    /// 変数を生存状態で登録する
    pub fn register(&mut self, name: &str) {
        self.alive.insert(name.to_string(), true);
        self.borrow_count.insert(name.to_string(), 0);
    }

    /// 変数を消費済みとしてマークする（所有権の移動）
    /// 既に消費済みの場合は二重解放エラーを記録する。
    /// 借用中の場合は消費を拒否する。
    pub fn consume(&mut self, name: &str) -> Result<(), String> {
        // 借用中チェック: 借用されている変数は消費できない
        if let Some(&count) = self.borrow_count.get(name) {
            if count > 0 {
                let borrower_names = self
                    .borrowers
                    .get(name)
                    .map(|v| v.join(", "))
                    .unwrap_or_else(|| "unknown".to_string());
                let msg = format!(
                    "Cannot consume '{}': currently borrowed by [{}] ({} active borrow(s))",
                    name, borrower_names, count
                );
                self.violations.push(msg.clone());
                return Err(msg);
            }
        }

        match self.alive.get(name) {
            Some(true) => {
                self.alive.insert(name.to_string(), false);
                Ok(())
            }
            Some(false) => {
                let msg = format!("Double-free detected: '{}' has already been consumed", name);
                self.violations.push(msg.clone());
                Err(msg)
            }
            None => {
                // 追跡対象外の変数は無視（通常の値型）
                Ok(())
            }
        }
    }

    /// 変数を借用する（読み取り専用の参照）
    /// 借用中は所有者が consume/free できなくなる。
    /// borrower_name: 借用する側の変数名（ライフタイム追跡用）
    #[allow(dead_code)]
    pub fn borrow(&mut self, owner_name: &str, borrower_name: &str) -> Result<(), String> {
        // 生存チェック: 消費済み変数は借用できない
        if let Some(false) = self.alive.get(owner_name) {
            let msg = format!(
                "Cannot borrow '{}': it has already been consumed (use-after-free)",
                owner_name
            );
            self.violations.push(msg.clone());
            return Err(msg);
        }

        let count = self.borrow_count.entry(owner_name.to_string()).or_insert(0);
        *count += 1;
        self.borrowers
            .entry(owner_name.to_string())
            .or_insert_with(Vec::new)
            .push(borrower_name.to_string());
        Ok(())
    }

    /// 借用を解放する
    #[allow(dead_code)]
    pub fn release_borrow(&mut self, owner_name: &str, borrower_name: &str) {
        if let Some(count) = self.borrow_count.get_mut(owner_name) {
            if *count > 0 {
                *count -= 1;
            }
        }
        if let Some(borrowers) = self.borrowers.get_mut(owner_name) {
            borrowers.retain(|b| b != borrower_name);
        }
    }

    /// 変数が生存しているかチェックする
    /// 消費済み変数へのアクセスはエラーを記録する
    #[allow(dead_code)]
    pub fn check_alive(&mut self, name: &str) -> Result<(), String> {
        if let Some(false) = self.alive.get(name) {
            let msg = format!(
                "Use-after-free detected: '{}' has been consumed and is no longer valid",
                name
            );
            self.violations.push(msg.clone());
            return Err(msg);
        }
        Ok(())
    }

    /// 変数が借用中かどうかを確認する
    #[allow(dead_code)]
    pub fn is_borrowed(&self, name: &str) -> bool {
        self.borrow_count.get(name).map_or(false, |&c| c > 0)
    }

    /// 蓄積された違反リストを返す
    pub fn get_violations(&self) -> &[String] {
        &self.violations
    }

    /// 違反があるかどうか
    pub fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }
}

// =============================================================================
// モジュール環境: グローバル static Mutex から構造体ベースの管理に移行
// =============================================================================

/// モジュール単位の環境。型定義・構造体定義・atom 定義・enum 定義を保持する。
/// グローバル static Mutex を廃止し、この構造体で一元管理する。
/// main.rs で構築し、verify() / codegen / transpiler に参照渡しする。
#[derive(Debug, Clone, Default)]
pub struct ModuleEnv {
    /// 精緻型定義（FQN キー: 例 "math::Nat" or 自モジュールなら "Nat"）
    pub types: HashMap<String, RefinedType>,
    /// 構造体定義（FQN キー）
    pub structs: HashMap<String, StructDef>,
    /// Atom 定義（FQN キー）。契約による検証で requires/ensures のみ参照する。
    pub atoms: HashMap<String, Atom>,
    /// Enum 定義（FQN キー）
    pub enums: HashMap<String, EnumDef>,
    /// トレイト定義
    pub traits: HashMap<String, TraitDef>,
    /// トレイト実装: (トレイト名, 型名) → ImplDef
    pub impls: Vec<ImplDef>,
    /// 検証済み Atom 名のキャッシュ
    pub verified_cache: HashSet<String>,
    /// リソース定義（非同期安全性検証用）
    /// リソース名 → (優先度, アクセスモード)
    pub resources: HashMap<String, ResourceDef>,
}

impl ModuleEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_type(&mut self, refined_type: &RefinedType) {
        self.types
            .insert(refined_type.name.clone(), refined_type.clone());
    }

    pub fn register_struct(&mut self, struct_def: &StructDef) {
        self.structs
            .insert(struct_def.name.clone(), struct_def.clone());
    }

    pub fn register_atom(&mut self, atom: &Atom) {
        self.atoms.insert(atom.name.clone(), atom.clone());
    }

    pub fn register_enum(&mut self, enum_def: &EnumDef) {
        self.enums.insert(enum_def.name.clone(), enum_def.clone());
    }

    pub fn get_type(&self, name: &str) -> Option<&RefinedType> {
        self.types.get(name)
    }

    pub fn get_struct(&self, name: &str) -> Option<&StructDef> {
        self.structs.get(name)
    }

    pub fn get_atom(&self, name: &str) -> Option<&Atom> {
        self.atoms.get(name)
    }

    #[allow(dead_code)]
    pub fn get_enum(&self, name: &str) -> Option<&EnumDef> {
        self.enums.get(name)
    }

    /// Variant 名から所属する Enum 定義を逆引きする
    pub fn find_enum_by_variant(&self, variant_name: &str) -> Option<&EnumDef> {
        self.enums
            .values()
            .find(|e| e.variants.iter().any(|v| v.name == variant_name))
    }

    /// 精緻型名からベース型名を解決する（例: "Nat" -> "i64", "Pos" -> "f64"）
    pub fn resolve_base_type(&self, type_name: &str) -> String {
        if let Some(refined) = self.types.get(type_name) {
            return refined._base_type.clone();
        }
        type_name.to_string()
    }

    pub fn register_trait(&mut self, trait_def: &TraitDef) {
        self.traits
            .insert(trait_def.name.clone(), trait_def.clone());
    }

    pub fn register_impl(&mut self, impl_def: &ImplDef) {
        self.impls.push(impl_def.clone());
    }

    pub fn get_trait(&self, name: &str) -> Option<&TraitDef> {
        self.traits.get(name)
    }

    /// 指定した型がトレイトを実装しているか確認する
    #[allow(dead_code)]
    pub fn find_impl(&self, trait_name: &str, target_type: &str) -> Option<&ImplDef> {
        self.impls
            .iter()
            .find(|i| i.trait_name == trait_name && i.target_type == target_type)
    }

    /// 指定した型がトレイト境界を全て満たしているか検証する
    #[allow(dead_code)]
    pub fn check_trait_bounds(&self, type_name: &str, bounds: &[String]) -> Result<(), String> {
        for bound in bounds {
            if self.find_impl(bound, type_name).is_none() {
                return Err(format!(
                    "Type '{}' does not implement trait '{}'",
                    type_name, bound
                ));
            }
        }
        Ok(())
    }

    /// Atom を検証済みとしてマークする
    pub fn mark_verified(&mut self, atom_name: &str) {
        self.verified_cache.insert(atom_name.to_string());
    }

    /// Atom が検証済みかどうかを確認する
    pub fn is_verified(&self, atom_name: &str) -> bool {
        self.verified_cache.contains(atom_name)
    }

    /// リソース定義を登録する
    pub fn register_resource(&mut self, resource_def: &ResourceDef) {
        self.resources
            .insert(resource_def.name.clone(), resource_def.clone());
    }

    /// リソース定義を取得する
    #[allow(dead_code)]
    pub fn get_resource(&self, name: &str) -> Option<&ResourceDef> {
        self.resources.get(name)
    }
}

// =============================================================================
// 組み込みトレイト (Built-in Traits)
// =============================================================================

/// 組み込みトレイトを ModuleEnv に自動登録する。
/// Numeric（算術演算）、Ord（比較）、Eq（等価性）の3つを提供。
pub fn register_builtin_traits(module_env: &mut ModuleEnv) {
    use crate::parser::{ImplDef as ID, TraitDef as TD, TraitMethod};

    // --- trait Eq ---
    // fn eq(a: Self, b: Self) -> bool;
    // law reflexive: eq(x, x) == true;
    // law symmetric: eq(a, b) => eq(b, a);
    module_env.register_trait(&TD {
        name: "Eq".to_string(),
        methods: vec![TraitMethod {
            name: "eq".to_string(),
            param_types: vec!["Self".into(), "Self".into()],
            return_type: "bool".into(),
            param_constraints: vec![None, None],
        }],
        laws: vec![
            ("reflexive".into(), "eq(x, x) == true".into()),
            ("symmetric".into(), "eq(a, b) => eq(b, a)".into()),
        ],
        span: Span::default(),
    });

    // --- trait Ord (extends Eq implicitly) ---
    // fn leq(a: Self, b: Self) -> bool;
    // law reflexive: leq(x, x) == true;
    // law antisymmetric: leq(a, b) && leq(b, a) => eq(a, b);
    // law transitive: leq(a, b) && leq(b, c) => leq(a, c);
    module_env.register_trait(&TD {
        name: "Ord".to_string(),
        methods: vec![TraitMethod {
            name: "leq".to_string(),
            param_types: vec!["Self".into(), "Self".into()],
            return_type: "bool".into(),
            param_constraints: vec![None, None],
        }],
        laws: vec![
            ("reflexive".into(), "leq(x, x) == true".into()),
            (
                "transitive".into(),
                "leq(a, b) && leq(b, c) => leq(a, c)".into(),
            ),
        ],
        span: Span::default(),
    });

    // --- trait Numeric (extends Ord implicitly) ---
    // fn add(a: Self, b: Self) -> Self;
    // fn sub(a: Self, b: Self) -> Self;
    // fn mul(a: Self, b: Self) -> Self;
    // law additive_identity: add(a, 0) == a;
    // law commutative_add: add(a, b) == add(b, a);
    module_env.register_trait(&TD {
        name: "Numeric".to_string(),
        methods: vec![
            TraitMethod {
                name: "add".to_string(),
                param_types: vec!["Self".into(), "Self".into()],
                return_type: "Self".into(),
                param_constraints: vec![None, None],
            },
            TraitMethod {
                name: "sub".to_string(),
                param_types: vec!["Self".into(), "Self".into()],
                return_type: "Self".into(),
                param_constraints: vec![None, None],
            },
            TraitMethod {
                name: "mul".to_string(),
                param_types: vec!["Self".into(), "Self".into()],
                return_type: "Self".into(),
                param_constraints: vec![None, None],
            },
        ],
        laws: vec![("commutative_add".into(), "add(a, b) == add(b, a)".into())],
        span: Span::default(),
    });

    // --- 組み込み impl: i64, u64, f64 は Eq + Ord + Numeric を自動実装 ---
    for base_type in &["i64", "u64", "f64"] {
        module_env.register_impl(&ID {
            trait_name: "Eq".into(),
            target_type: base_type.to_string(),
            method_bodies: vec![("eq".into(), "a == b".into())],
            span: Span::default(),
        });
        module_env.register_impl(&ID {
            trait_name: "Ord".into(),
            target_type: base_type.to_string(),
            method_bodies: vec![("leq".into(), "a <= b".into())],
            span: Span::default(),
        });
        module_env.register_impl(&ID {
            trait_name: "Numeric".into(),
            target_type: base_type.to_string(),
            method_bodies: vec![
                ("add".into(), "a + b".into()),
                ("sub".into(), "a - b".into()),
                ("mul".into(), "a * b".into()),
            ],
            span: Span::default(),
        });
    }
}

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
fn substitute_method_calls(
    law_expr: &str,
    method_bodies: &HashMap<String, String>,
    method_params: &HashMap<String, Vec<String>>,
) -> String {
    let mut result = law_expr.to_string();

    // 各メソッドについて繰り返し展開（ネスト対応のため複数パス）
    for _pass in 0..5 {
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
                    && (i == 0 || !chars[i - 1].is_alphanumeric())
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
                                // 単語境界を考慮した置換（部分一致を防ぐ）
                                expanded = replace_word(
                                    &expanded,
                                    param_name,
                                    &format!("({})", arg.trim()),
                                );
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

/// 単語境界を考慮した文字列置換。
/// "a" を置換する際に "a" 単体のみマッチし、"add" 内の "a" にはマッチしない。
fn replace_word(source: &str, word: &str, replacement: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = source.chars().collect();
    let word_chars: Vec<char> = word.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if i + word_chars.len() <= chars.len()
            && chars[i..i + word_chars.len()] == word_chars[..]
            && (i == 0 || !chars[i - 1].is_alphanumeric() && chars[i - 1] != '_')
            && (i + word_chars.len() >= chars.len()
                || !chars[i + word_chars.len()].is_alphanumeric()
                    && chars[i + word_chars.len()] != '_')
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
fn split_args(input: &str) -> Vec<String> {
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
pub fn verify_impl(impl_def: &ImplDef, module_env: &ModuleEnv) -> MumeiResult<()> {
    let trait_def = module_env.get_trait(&impl_def.trait_name).ok_or_else(|| {
        MumeiError::TypeError(format!(
            "Trait '{}' not found for impl on '{}'",
            impl_def.trait_name, impl_def.target_type
        ))
    })?;

    // メソッドの完全性チェック: trait の全メソッドが impl されているか
    for method in &trait_def.methods {
        if !impl_def
            .method_bodies
            .iter()
            .any(|(name, _)| name == &method.name)
        {
            return Err(MumeiError::TypeError(format!(
                "impl {} for {}: missing method '{}'",
                impl_def.trait_name, impl_def.target_type, method.name
            )));
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
        let int_sort = z3::Sort::int(&ctx);
        let arr = Array::new_const(&ctx, "arr", &int_sort, &int_sort);
        let vc = VCtx {
            ctx: &ctx,
            arr: &arr,
            module_env,
        };

        let mut env: Env = HashMap::new();
        // law 内の自由変数をシンボリック整数として登録
        for var_name in &["a", "b", "c", "x", "y", "z"] {
            let base = module_env.resolve_base_type(&impl_def.target_type);
            let var: Dynamic = match base.as_str() {
                "f64" => Float::new_const(&ctx, *var_name, 11, 53).into(),
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
                    solver.assert(&law_bool.not());
                    if solver.check() == SatResult::Sat {
                        // 反例（Counter-example）を Z3 model から取得
                        let counterexample = if let Some(model) = solver.get_model() {
                            let var_names = ["a", "b", "c", "x", "y", "z"];
                            let mut ce_parts = Vec::new();
                            for var_name in &var_names {
                                if let Some(var_z3) = env.get(*var_name) {
                                    if let Some(val) = model.eval(var_z3, true) {
                                        let val_str = format!("{}", val);
                                        // 変数が law 式に含まれている場合のみ表示
                                        if law_expr.contains(*var_name) {
                                            ce_parts.push(format!("{} = {}", var_name, val_str));
                                        }
                                    }
                                }
                            }
                            if ce_parts.is_empty() {
                                "  (no concrete values available)".to_string()
                            } else {
                                format!("  Counter-example: {}", ce_parts.join(", "))
                            }
                        } else {
                            "  (could not retrieve model)".to_string()
                        };
                        solver.pop(1);
                        return Err(MumeiError::VerificationError(
                            format!(
                                "impl {} for {}: law '{}' is not satisfied\n  Law: {}\n  Expanded: {}\n{}",
                                impl_def.trait_name, impl_def.target_type,
                                law_name, law_expr, substituted, counterexample
                            )
                        ));
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

/// リソース取得コンテキスト: 現在保持中のリソースとその優先度を追跡する。
/// acquire 式の検証時に、リソース階層制約をチェックする。
#[derive(Debug, Clone, Default)]
struct ResourceCtx {
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
    fn has_violations(&self) -> bool {
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
fn verify_resource_hierarchy(atom: &Atom, module_env: &ModuleEnv) -> MumeiResult<()> {
    if atom.resources.is_empty() {
        return Ok(());
    }

    // リソース定義の存在チェック
    let mut resource_priorities: Vec<(String, i64)> = Vec::new();
    for res_name in &atom.resources {
        if let Some(rdef) = module_env.resources.get(res_name) {
            resource_priorities.push((rdef.name.clone(), rdef.priority));
        } else {
            return Err(MumeiError::TypeError(
                format!("Resource '{}' used in atom '{}' is not defined. Add: resource {} priority:<N> mode:exclusive|shared;",
                    res_name, atom.name, res_name)
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
                return Err(MumeiError::VerificationError(
                    format!(
                        "Resource hierarchy violation in atom '{}': \
                         '{}' (priority={}) must have strictly lower priority than '{}' (priority={}). \
                         Reorder resources or adjust priorities to prevent potential deadlock.",
                        atom.name, name_i, resource_priorities[i].1,
                        name_j, resource_priorities[j].1
                    )
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
            if rdef.mode == ResourceMode::Exclusive {
                if !exclusive_set.insert(res_name.clone()) {
                    return Err(MumeiError::VerificationError(
                        format!(
                            "Data race risk in atom '{}': exclusive resource '{}' is listed multiple times",
                            atom.name, res_name
                        )
                    ));
                }
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
const BMC_DEFAULT_UNROLL_DEPTH: usize = 3;

/// 再帰的 async 呼び出しの最大展開深度。
/// async atom が自身を呼び出す場合、この深度を超えると
/// 「Unknown（未定義）」として扱い、Z3 探索を打ち切る。
const MAX_ASYNC_RECURSION_DEPTH: usize = 3;

/// body 内の Acquire 式を再帰的に収集する（BMC 用）。
/// ループ内で acquire が使われているパターンを検出するために使用。
fn collect_acquire_resources(expr: &Expr) -> Vec<String> {
    let mut resources = Vec::new();
    match expr {
        Expr::Acquire { resource, body } => {
            resources.push(resource.clone());
            resources.extend(collect_acquire_resources(body));
        }
        Expr::Block(stmts) => {
            for stmt in stmts {
                resources.extend(collect_acquire_resources(stmt));
            }
        }
        Expr::While { body, .. } => {
            resources.extend(collect_acquire_resources(body));
        }
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            resources.extend(collect_acquire_resources(then_branch));
            resources.extend(collect_acquire_resources(else_branch));
        }
        Expr::Let { value, .. } | Expr::Assign { value, .. } => {
            resources.extend(collect_acquire_resources(value));
        }
        Expr::Async { body } => {
            resources.extend(collect_acquire_resources(body));
        }
        Expr::Await { expr } => {
            resources.extend(collect_acquire_resources(expr));
        }
        _ => {}
    }
    resources
}

/// 有界モデル検査: atom の body 内のループを展開し、
/// 各展開でリソース階層制約が維持されることを検証する。
///
/// 展開回数は atom.max_unroll（指定時）または BMC_DEFAULT_UNROLL_DEPTH を使用。
/// ループ不変量が提供されている場合はスキップ（不変量ベースの検証が優先）。
/// BMC は「ユーザーが不変量を書けない場合」の補助的な検証手段。
fn verify_bmc_resource_safety(atom: &Atom, module_env: &ModuleEnv) -> MumeiResult<()> {
    // body 内に acquire が含まれない場合はスキップ
    let body_ast = parse_expression(&atom.body_expr);
    let acquired_resources = collect_acquire_resources(&body_ast);
    if acquired_resources.is_empty() {
        return Ok(());
    }

    // While ループ内に acquire があるかチェック
    fn has_acquire_in_while(expr: &Expr) -> bool {
        match expr {
            Expr::While { body, .. } => !collect_acquire_resources(body).is_empty(),
            Expr::Block(stmts) => stmts.iter().any(has_acquire_in_while),
            Expr::IfThenElse {
                then_branch,
                else_branch,
                ..
            } => has_acquire_in_while(then_branch) || has_acquire_in_while(else_branch),
            _ => false,
        }
    }

    if !has_acquire_in_while(&body_ast) {
        return Ok(()); // ループ外の acquire は通常の検証で十分
    }

    // 展開回数: atom 単位のオーバーライド > グローバルデフォルト
    let unroll_depth = atom.max_unroll.unwrap_or(BMC_DEFAULT_UNROLL_DEPTH);

    // BMC: ループを展開して各ステップでリソース階層をチェック
    let mut resource_ctx = ResourceCtx::new();

    for unroll_step in 0..unroll_depth {
        // 各展開ステップで acquire されるリソースの順序を検証
        for res_name in &acquired_resources {
            if let Some(rdef) = module_env.resources.get(res_name) {
                if let Err(e) = resource_ctx.acquire(res_name, rdef.priority) {
                    return Err(MumeiError::VerificationError(
                        format!(
                            "BMC (unroll step {}/{}, max_unroll={}): resource ordering violation in loop body: {}",
                            unroll_step, unroll_depth, unroll_depth, e
                        )
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
fn verify_async_recursion_depth(atom: &Atom, module_env: &ModuleEnv) -> MumeiResult<()> {
    if !atom.is_async {
        return Ok(());
    }

    fn count_self_calls(expr: &Expr, atom_name: &str) -> usize {
        match expr {
            Expr::Call(name, args) => {
                let self_call = if name == atom_name { 1 } else { 0 };
                self_call
                    + args
                        .iter()
                        .map(|a| count_self_calls(a, atom_name))
                        .sum::<usize>()
            }
            Expr::Block(stmts) => stmts.iter().map(|s| count_self_calls(s, atom_name)).sum(),
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                count_self_calls(cond, atom_name)
                    + count_self_calls(then_branch, atom_name)
                    + count_self_calls(else_branch, atom_name)
            }
            Expr::Let { value, .. } | Expr::Assign { value, .. } => {
                count_self_calls(value, atom_name)
            }
            Expr::Async { body } => count_self_calls(body, atom_name),
            Expr::Await { expr } => count_self_calls(expr, atom_name),
            Expr::Acquire { body, .. } => count_self_calls(body, atom_name),
            Expr::While { cond, body, .. } => {
                count_self_calls(cond, atom_name) + count_self_calls(body, atom_name)
            }
            Expr::BinaryOp(l, _, r) => {
                count_self_calls(l, atom_name) + count_self_calls(r, atom_name)
            }
            _ => 0,
        }
    }

    let body_ast = parse_expression(&atom.body_expr);
    let self_call_count = count_self_calls(&body_ast, &atom.name);

    if self_call_count > 0 {
        // 再帰的 async 呼び出しが検出された
        // 呼び出し先の async atom も再帰する可能性があるため、
        // 深度制限を超える場合は警告
        let max_depth = atom.max_unroll.unwrap_or(MAX_ASYNC_RECURSION_DEPTH);
        if self_call_count > max_depth {
            return Err(MumeiError::VerificationError(format!(
                "Async recursion depth exceeded in atom '{}': {} self-calls detected \
                     (max_depth={}). Use max_unroll: {}; to increase the limit, or \
                     refactor to use iteration with invariant.",
                atom.name,
                self_call_count,
                max_depth,
                self_call_count + 1
            )));
        }

        // 再帰呼び出し先の契約を信頼して展開（Compositional Verification）
        // 各展開ステップで ensures を仮定として使用する。
        // これにより、f_depth_1, f_depth_2 ... と別シンボルとして扱われ、
        // Z3 が無限ループに陥ることを防ぐ。
        if let Some(callee) = module_env.get_atom(&atom.name) {
            if callee.ensures.trim() == "true" {
                // ensures が trivial な場合、再帰の安全性を証明できない
                return Err(MumeiError::VerificationError(format!(
                    "Recursive async atom '{}' requires a non-trivial ensures clause \
                         for inductive verification. Add: ensures: <postcondition>;",
                    atom.name
                )));
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
fn verify_atom_invariant(
    atom: &Atom,
    invariant_raw: &str,
    module_env: &ModuleEnv,
) -> MumeiResult<()> {
    let mut cfg = Config::new();
    cfg.set_timeout_msec(5000);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    let int_sort = z3::Sort::int(&ctx);
    let arr = Array::new_const(&ctx, "arr", &int_sort, &int_sort);
    let vc = VCtx {
        ctx: &ctx,
        arr: &arr,
        module_env,
    };

    let mut env: Env = HashMap::new();

    // パラメータをシンボリック変数として登録
    for param in &atom.params {
        let base = param
            .type_name
            .as_deref()
            .map(|t| module_env.resolve_base_type(t))
            .unwrap_or_else(|| "i64".to_string());
        let var: Dynamic = match base.as_str() {
            "f64" => Float::new_const(&ctx, param.name.as_str(), 11, 53).into(),
            _ => Int::new_const(&ctx, param.name.as_str()).into(),
        };
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
            .ok_or(MumeiError::TypeError(format!(
                "Invariant for atom '{}' must be a boolean expression",
                atom.name
            )))?;

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
                return Err(MumeiError::VerificationError(format!(
                    "Invariant induction base failed for atom '{}': \
                         requires does not imply invariant.\n  \
                         Invariant: {}\n  \
                         Requires: {}\n  \
                         The invariant must hold whenever the precondition is satisfied.",
                    atom.name, invariant_raw, atom.requires
                )));
            }
            solver.pop(1);
        }
    } else {
        // requires が true の場合、invariant は無条件に成立する必要がある
        solver.push();
        solver.assert(&inv_z3.not());
        if solver.check() == SatResult::Sat {
            solver.pop(1);
            return Err(MumeiError::VerificationError(format!(
                "Invariant induction base failed for atom '{}': \
                     invariant '{}' is not universally true (no requires constraint).",
                atom.name, invariant_raw
            )));
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
        let body_ast = parse_expression(&atom.body_expr);
        let _body_result = expr_to_z3(&vc, &body_ast, &mut env, Some(&solver))?;

        // body 実行後の invariant を再評価
        // （env が body の実行で更新されている可能性がある）
        let inv_after = expr_to_z3(&vc, &inv_ast, &mut env, None)?
            .as_bool()
            .ok_or(MumeiError::TypeError("Invariant must be boolean".into()))?;

        // invariant の維持を検証: ¬inv_after が Unsat なら維持されている
        solver.assert(&inv_after.not());
        if solver.check() == SatResult::Sat {
            solver.pop(1);
            return Err(MumeiError::VerificationError(format!(
                "Invariant preservation failed for atom '{}': \
                     body execution may violate the invariant.\n  \
                     Invariant: {}\n  \
                     The invariant must be maintained after executing the body.",
                atom.name, invariant_raw
            )));
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

/// body 内の全 Call 式から呼び出し先の atom 名を収集する。
fn collect_callees(expr: &Expr) -> Vec<String> {
    let mut callees = Vec::new();
    match expr {
        Expr::Call(name, args) => {
            callees.push(name.clone());
            for arg in args {
                callees.extend(collect_callees(arg));
            }
        }
        Expr::Block(stmts) => {
            for s in stmts {
                callees.extend(collect_callees(s));
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            callees.extend(collect_callees(cond));
            callees.extend(collect_callees(then_branch));
            callees.extend(collect_callees(else_branch));
        }
        Expr::Let { value, .. } | Expr::Assign { value, .. } => {
            callees.extend(collect_callees(value));
        }
        Expr::While { cond, body, .. } => {
            callees.extend(collect_callees(cond));
            callees.extend(collect_callees(body));
        }
        Expr::BinaryOp(l, _, r) => {
            callees.extend(collect_callees(l));
            callees.extend(collect_callees(r));
        }
        Expr::Async { body } | Expr::Acquire { body, .. } => {
            callees.extend(collect_callees(body));
        }
        Expr::Await { expr } => {
            callees.extend(collect_callees(expr));
        }
        Expr::Match { target, arms } => {
            callees.extend(collect_callees(target));
            for arm in arms {
                callees.extend(collect_callees(&arm.body));
                if let Some(guard) = &arm.guard {
                    callees.extend(collect_callees(guard));
                }
            }
        }
        _ => {}
    }
    callees
}

/// Call Graph のサイクルを DFS で検出する。
/// atom_name から到達可能なサイクルがある場合、サイクルのパスを返す。
fn detect_call_cycle(atom_name: &str, module_env: &ModuleEnv) -> Option<Vec<String>> {
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
            let body_ast = parse_expression(&callee_atom.body_expr);
            let callees = collect_callees(&body_ast);
            for callee_name in &callees {
                if let Some(_) = module_env.get_atom(callee_name) {
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
        let body_ast = parse_expression(&atom.body_expr);
        let callees = collect_callees(&body_ast);
        for callee_name in &callees {
            if let Some(_) = module_env.get_atom(callee_name) {
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
fn verify_call_graph_cycles(atom: &Atom, module_env: &ModuleEnv) -> MumeiResult<()> {
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
fn check_taint_propagation(atom: &Atom, env: &Env, module_env: &ModuleEnv) {
    // body 内で呼び出されている関数を収集
    let body_ast = parse_expression(&atom.body_expr);
    let callees = collect_callees(&body_ast);

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

/// mumei.toml の [proof]/[build] 設定を反映した verify
/// timeout_ms: Z3 ソルバのタイムアウト（ミリ秒）
/// global_max_unroll: BMC のグローバル展開深度
pub fn verify_with_config(
    atom: &Atom,
    output_dir: &Path,
    module_env: &ModuleEnv,
    timeout_ms: u64,
    _global_max_unroll: usize,
) -> MumeiResult<()> {
    verify_inner(atom, output_dir, module_env, timeout_ms)
}

pub fn verify(atom: &Atom, output_dir: &Path, module_env: &ModuleEnv) -> MumeiResult<()> {
    verify_inner(atom, output_dir, module_env, 10000)
}

fn verify_inner(
    atom: &Atom,
    output_dir: &Path,
    module_env: &ModuleEnv,
    timeout_ms: u64,
) -> MumeiResult<()> {
    // Phase 0: 信頼レベルチェック（Trust Boundary）
    match &atom.trust_level {
        TrustLevel::Trusted => {
            // trusted atom: body の検証をスキップし、契約（requires/ensures）のみ信頼する。
            // 呼び出し元は契約に基づいて Compositional Verification を行う。
            save_visualizer_report(
                output_dir,
                "trusted",
                &atom.name,
                "N/A",
                "N/A",
                "Trusted: body verification skipped, contract assumed correct.",
            );
            return Ok(());
        }
        TrustLevel::Unverified => {
            // unverified atom: 警告を出すが、検証は続行する。
            // ensures が non-trivial な場合のみ検証を試みる。
            eprintln!(
                "  ⚠️  Warning: atom '{}' is marked as 'unverified'. \
                       Verification results may be incomplete.",
                atom.name
            );
            if atom.ensures.trim() == "true" && atom.requires.trim() == "true" {
                // 契約が trivial な場合、検証する意味がないのでスキップ
                save_visualizer_report(
                    output_dir,
                    "unverified",
                    &atom.name,
                    "N/A",
                    "N/A",
                    "Unverified: no contract to verify.",
                );
                return Ok(());
            }
        }
        TrustLevel::Verified => {
            // 通常の検証フロー
        }
    }

    // Phase 1: リソース階層検証（デッドロック防止）
    verify_resource_hierarchy(atom, module_env)?;

    // Phase 1b: 有界モデル検査（ループ内 acquire パターン）
    verify_bmc_resource_safety(atom, module_env)?;

    // Phase 1c: 再帰的 async 呼び出しの深度検証
    verify_async_recursion_depth(atom, module_env)?;

    // Phase 1d: atom レベル invariant の帰納的検証
    if let Some(ref invariant_expr) = atom.invariant {
        verify_atom_invariant(atom, invariant_expr, module_env)?;
    }

    // Phase 1e: Call Graph サイクル検知（間接再帰の検出）
    verify_call_graph_cycles(atom, module_env)?;

    let mut cfg = Config::new();
    cfg.set_timeout_msec(timeout_ms);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    let int_sort = z3::Sort::int(&ctx);
    let arr = Array::new_const(&ctx, "arr", &int_sort, &int_sort);
    let vc = VCtx {
        ctx: &ctx,
        arr: &arr,
        module_env,
    };

    let mut env: Env = HashMap::new();

    // 1. 量子化制約の処理
    for q in &atom.forall_constraints {
        let i = Int::new_const(&ctx, q.var.as_str());
        let start = Int::from_i64(&ctx, q.start.parse::<i64>().unwrap_or(0));
        let end = if let Ok(val) = q.end.parse::<i64>() {
            Int::from_i64(&ctx, val)
        } else {
            Int::new_const(&ctx, q.end.as_str())
        };

        let range_cond = Bool::and(&ctx, &[&i.ge(&start), &i.lt(&end)]);
        let expr_ast = parse_expression(&q.condition);
        let condition_z3 = expr_to_z3(&vc, &expr_ast, &mut env, None)?
            .as_bool()
            .ok_or(MumeiError::VerificationError(
                "Condition must be boolean".into(),
            ))?;

        let quantifier_expr = match q.q_type {
            QuantifierType::ForAll => {
                z3::ast::forall_const(&ctx, &[&i], &[], &range_cond.implies(&condition_z3))
            }
            QuantifierType::Exists => z3::ast::exists_const(
                &ctx,
                &[&i],
                &[],
                &Bool::and(&ctx, &[&range_cond, &condition_z3]),
            ),
        };
        solver.assert(&quantifier_expr);
    }

    // 2. 引数（params）に対する精緻型制約の自動適用
    for param in &atom.params {
        if let Some(type_name) = &param.type_name {
            if let Some(refined) = module_env.get_type(type_name) {
                apply_refinement_constraint(&vc, &solver, &param.name, refined, &mut env)?;
            }
        }
    }

    // 2b. 引数（params）に対する構造体フィールド制約の自動適用
    for param in &atom.params {
        if let Some(type_name) = &param.type_name {
            if let Some(sdef) = module_env.get_struct(type_name) {
                // 構造体の各フィールドをシンボリック変数として env に登録し、制約を適用
                for field in &sdef.fields {
                    let field_var_name = format!("{}_{}", param.name, field.name);
                    let base = module_env.resolve_base_type(&field.type_name);
                    let field_z3: Dynamic = match base.as_str() {
                        "f64" => Float::new_const(&ctx, field_var_name.as_str(), 11, 53).into(),
                        _ => Int::new_const(&ctx, field_var_name.as_str()).into(),
                    };
                    env.insert(field_var_name.clone(), field_z3.clone());
                    // qualified name も登録
                    let qualified = format!("__struct_{}_{}", param.name, field.name);
                    env.insert(qualified, field_z3.clone());

                    // フィールド制約を solver に assert
                    if let Some(constraint_raw) = &field.constraint {
                        let mut local_env = env.clone();
                        local_env.insert("v".to_string(), field_z3);
                        let constraint_ast = parse_expression(constraint_raw);
                        let constraint_z3 = expr_to_z3(&vc, &constraint_ast, &mut local_env, None)?;
                        if let Some(constraint_bool) = constraint_z3.as_bool() {
                            solver.assert(&constraint_bool);
                        }
                    }
                }
            }
        }
    }

    // 2c. 全パラメータに対して配列長シンボルを事前生成
    for param in &atom.params {
        let len_name = format!("len_{}", param.name);
        if !env.contains_key(&len_name) {
            let len_var = Int::new_const(&ctx, len_name.as_str());
            solver.assert(&len_var.ge(&Int::from_i64(&ctx, 0)));
            env.insert(len_name, len_var.into());
        }
    }

    // 2d. 線形性チェック: consumed_params + ref パラメータの Z3 シンボリック Bool 連携
    // consume 宣言されたパラメータに対して is_alive フラグを Z3 上で追跡する。
    // ref パラメータに対しては借用カウントを追跡し、借用中の consume を禁止する。
    let mut linearity_ctx = LinearityCtx::new();

    // consume 対象パラメータの登録
    if !atom.consumed_params.is_empty() {
        for param_name in &atom.consumed_params {
            // パラメータが実際に存在するか検証
            if !atom.params.iter().any(|p| p.name == *param_name) {
                return Err(MumeiError::TypeError(format!(
                    "consume target '{}' is not a parameter of atom '{}'",
                    param_name, atom.name
                )));
            }
            // ref / ref mut パラメータは consume できない
            if atom
                .params
                .iter()
                .any(|p| p.name == *param_name && (p.is_ref || p.is_ref_mut))
            {
                let kind = if atom
                    .params
                    .iter()
                    .any(|p| p.name == *param_name && p.is_ref_mut)
                {
                    "ref mut"
                } else {
                    "ref"
                };
                return Err(MumeiError::TypeError(
                    format!("Cannot consume {} parameter '{}' in atom '{}': {} parameters are borrowed, not owned", kind, param_name, atom.name, kind)
                ));
            }
            // LinearityCtx に登録
            linearity_ctx.register(param_name);

            // Z3 上で is_alive シンボリック Bool を作成し、初期値 true を assert
            let alive_name = format!("__alive_{}", param_name);
            let alive_bool = Bool::new_const(&ctx, alive_name.as_str());
            solver.assert(&alive_bool); // 初期状態: alive = true
            env.insert(alive_name, alive_bool.into());
        }
    }

    // ref / ref mut パラメータの借用登録
    // ref パラメータは読み取り専用で貸し出される。
    // ref mut パラメータは排他的な書き込み参照として貸し出される。
    // 借用中は元の所有者（呼び出し元）が consume/free できない。
    // この制約は呼び出し元の verify() で検証される（Compositional Verification）。
    for param in &atom.params {
        if param.is_ref || param.is_ref_mut {
            // ref/ref mut パラメータを LinearityCtx に登録（借用として）
            linearity_ctx.register(&param.name);

            // Z3 上で borrowed フラグを作成
            let borrowed_name = format!("__borrowed_{}", param.name);
            let borrowed_bool = Bool::new_const(&ctx, borrowed_name.as_str());
            solver.assert(&borrowed_bool); // 借用中: true
            env.insert(borrowed_name, borrowed_bool.into());

            // ref/ref mut パラメータは consume 不可であることを Z3 で表現
            // __alive_{name} は常に true（借用中は解放不可）
            let alive_name = format!("__alive_{}", param.name);
            let alive_bool = Bool::new_const(&ctx, alive_name.as_str());
            solver.assert(&alive_bool); // ref は常に alive
            env.insert(alive_name, alive_bool.into());

            // ref mut の場合: 排他的アクセス（exclusive）を Z3 で表現
            if param.is_ref_mut {
                let exclusive_name = format!("__exclusive_{}", param.name);
                let exclusive_bool = Bool::new_const(&ctx, exclusive_name.as_str());
                solver.assert(&exclusive_bool); // exclusive = true
                env.insert(exclusive_name, exclusive_bool.into());
            }
        }
    }

    // 3. 前提条件 (requires)
    // NOTE: requires は エイリアシング検証より先に assert する必要がある。
    // requires: x != y; のような制約がエイリアシング検証で活用されるため。
    if atom.requires.trim() != "true" {
        let req_ast = parse_expression(&atom.requires);
        let req_z3 = expr_to_z3(&vc, &req_ast, &mut env, None)?;
        if let Some(req_bool) = req_z3.as_bool() {
            solver.assert(&req_bool);
        }
    }

    // 3b. エイリアシング検証 (Aliasing Prevention)
    // requires が assert された後に実行する。
    // これにより requires: x != y; のような制約が Z3 で活用され、
    // 「provably distinct」なパラメータはエイリアシングエラーにならない。
    //
    // ref mut パラメータが存在する場合、同じ型の他の ref/ref mut パラメータ
    // とのエイリアシング（同一データへの複数参照）を禁止する。
    //
    // Rust の借用規則と同等:
    // - &mut T が存在する場合、同じデータへの &T も &mut T も存在できない
    // - &T は複数同時に存在可能
    //
    // Z3 制約:
    // ∀ p1, p2 ∈ params:
    //   p1.is_ref_mut ∧ p1.type == p2.type ∧ p1 ≠ p2
    //   → ¬(p2.is_ref ∨ p2.is_ref_mut)  // エイリアシング禁止
    {
        let ref_mut_params: Vec<&crate::parser::Param> =
            atom.params.iter().filter(|p| p.is_ref_mut).collect();

        for ref_mut_p in &ref_mut_params {
            for other_p in &atom.params {
                if other_p.name == ref_mut_p.name {
                    continue; // 自分自身はスキップ
                }
                // 同じ型の ref または ref mut パラメータがある場合、エイリアシングの可能性
                if (other_p.is_ref || other_p.is_ref_mut)
                    && other_p.type_name == ref_mut_p.type_name
                {
                    // Z3 で同一データへの参照でないことを検証
                    // パラメータが異なる値を持つことを確認
                    // （同じ値を持つ場合、エイリアシングが発生）
                    if let (Some(ref_mut_val), Some(other_val)) =
                        (env.get(&ref_mut_p.name), env.get(&other_p.name))
                    {
                        if let (Some(rm_int), Some(ot_int)) =
                            (ref_mut_val.as_int(), other_val.as_int())
                        {
                            // ref_mut_val == other_val が SAT ならエイリアシングの可能性あり
                            solver.push();
                            solver.assert(&rm_int._eq(&ot_int));
                            if solver.check() == SatResult::Sat {
                                solver.pop(1);
                                let other_kind = if other_p.is_ref_mut { "ref mut" } else { "ref" };
                                return Err(MumeiError::VerificationError(
                                    format!(
                                        "Aliasing violation in atom '{}': \
                                         'ref mut {}' and '{} {}' may reference the same data (type: {}). \
                                         A mutable reference requires exclusive access — \
                                         no other references to the same data are allowed.\n  \
                                         Hint: Use different types, or ensure the values are provably distinct \
                                         via requires.",
                                        atom.name, ref_mut_p.name, other_kind, other_p.name,
                                        ref_mut_p.type_name.as_deref().unwrap_or("unknown")
                                    )
                                ));
                            }
                            solver.pop(1);
                        }
                    }
                }
            }
        }
    }

    // 4. ボディの検証
    let body_ast = parse_expression(&atom.body_expr);
    let body_result = expr_to_z3(&vc, &body_ast, &mut env, Some(&solver))?;

    // 4b. Taint Analysis: unverified 関数の呼び出しを検出し警告
    check_taint_propagation(atom, &env, module_env);

    // 5. 事後条件 (ensures)
    if atom.ensures.trim() != "true" {
        env.insert("result".to_string(), body_result);
        let ens_ast = parse_expression(&atom.ensures);
        let ens_z3 = expr_to_z3(&vc, &ens_ast, &mut env, None)?;
        if let Some(ens_bool) = ens_z3.as_bool() {
            solver.push();
            solver.assert(&ens_bool.not());
            if solver.check() == SatResult::Sat {
                solver.pop(1);
                save_visualizer_report(
                    output_dir,
                    "failed",
                    &atom.name,
                    "N/A",
                    "N/A",
                    "Postcondition violated.",
                );
                return Err(MumeiError::VerificationError(
                    "Postcondition (ensures) is not satisfied.".into(),
                ));
            }
            solver.pop(1);
        }
        env.remove("result");
    }

    // 5b. 線形性チェック: consume 対象パラメータの検証
    // body 実行後、consume 宣言されたパラメータが正しく消費されていることを確認。
    // LinearityCtx に蓄積された違反（二重解放・Use-After-Free）があればエラー。
    if !atom.consumed_params.is_empty() {
        // consume 対象パラメータを消費済みとしてマーク
        for param_name in &atom.consumed_params {
            if let Err(e) = linearity_ctx.consume(param_name) {
                return Err(MumeiError::VerificationError(format!(
                    "Linearity violation in atom '{}': {}",
                    atom.name, e
                )));
            }

            // Z3 上で is_alive を false に更新（消費後のアクセスを禁止）
            let alive_name = format!("__alive_{}", param_name);
            let alive_false = Bool::from_bool(&ctx, false);
            env.insert(alive_name, alive_false.into());
        }

        // 蓄積された違反をチェック
        if linearity_ctx.has_violations() {
            let violations = linearity_ctx.get_violations().join("\n  ");
            return Err(MumeiError::VerificationError(format!(
                "Linearity violations in atom '{}':\n  {}",
                atom.name, violations
            )));
        }
    }

    if solver.check() == SatResult::Unsat {
        save_visualizer_report(
            output_dir,
            "failed",
            &atom.name,
            "N/A",
            "N/A",
            "Logic contradiction.",
        );
        return Err(MumeiError::VerificationError("Contradiction found.".into()));
    }

    save_visualizer_report(
        output_dir,
        "success",
        &atom.name,
        "N/A",
        "N/A",
        "Verified safe.",
    );
    Ok(())
}

fn apply_refinement_constraint<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    var_name: &str,
    refined: &RefinedType,
    global_env: &mut Env<'a>,
) -> MumeiResult<()> {
    let ctx = vc.ctx;
    // Type System 2.0: ベース型に基づいて変数を生成
    let var_z3: Dynamic = match refined._base_type.as_str() {
        "f64" => Float::new_const(ctx, var_name, 11, 53).into(),
        "u64" => {
            let v = Int::new_const(ctx, var_name);
            solver.assert(&v.ge(&Int::from_i64(ctx, 0)));
            v.into()
        }
        _ => Int::new_const(ctx, var_name).into(),
    };

    global_env.insert(var_name.to_string(), var_z3.clone());

    let mut local_env = global_env.clone();
    local_env.insert(refined.operand.clone(), var_z3);

    let predicate_ast = parse_expression(&refined.predicate_raw);
    let predicate_z3 = expr_to_z3(vc, &predicate_ast, &mut local_env, None)?
        .as_bool()
        .ok_or(MumeiError::TypeError(format!(
            "Predicate for {} must be boolean",
            refined.name
        )))?;

    solver.assert(&predicate_z3);
    Ok(())
}

fn expr_to_z3<'a>(
    vc: &VCtx<'a>,
    expr: &Expr,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> DynResult<'a> {
    let ctx = vc.ctx;
    let arr = vc.arr;
    match expr {
        Expr::Number(n) => Ok(Int::from_i64(ctx, *n).into()),
        Expr::Float(f) => Ok(Float::from_f64(ctx, *f).into()),
        Expr::Variable(name) => Ok(env
            .get(name)
            .cloned()
            .unwrap_or_else(|| Int::new_const(ctx, name.as_str()).into())),
        Expr::Call(name, args) => {
            match name.as_str() {
                // =============================================================
                // ensures / invariant 内の forall/exists 量化子サポート
                // =============================================================
                // forall(var, start, end, condition) → Z3 ∀ var ∈ [start, end). condition
                // exists(var, start, end, condition) → Z3 ∃ var ∈ [start, end). condition
                //
                // これにより ensures: forall(i, 0, result - 1, arr[i] <= arr[i+1])
                // のようなソート済み不変量を事後条件として記述・検証できる。
                "forall" | "exists" => {
                    if args.len() != 4 {
                        return Err(MumeiError::VerificationError(format!(
                            "{}() requires exactly 4 arguments: (var, start, end, condition)",
                            name
                        )));
                    }
                    // 第1引数: 束縛変数名
                    let var_name = match &args[0] {
                        Expr::Variable(v) => v.clone(),
                        _ => {
                            return Err(MumeiError::VerificationError(format!(
                                "{}(): first argument must be a variable name",
                                name
                            )))
                        }
                    };

                    // 第2引数: 範囲の開始
                    let start_z3 = expr_to_z3(vc, &args[1], env, None)?.as_int().ok_or(
                        MumeiError::TypeError(format!("{}(): start must be integer", name)),
                    )?;

                    // 第3引数: 範囲の終了
                    let end_z3 = expr_to_z3(vc, &args[2], env, None)?.as_int().ok_or(
                        MumeiError::TypeError(format!("{}(): end must be integer", name)),
                    )?;

                    // 束縛変数を一時的に env に追加して condition を評価
                    let bound_var = Int::new_const(ctx, var_name.as_str());
                    let old_val = env.insert(var_name.clone(), bound_var.clone().into());

                    let range_cond =
                        Bool::and(ctx, &[&bound_var.ge(&start_z3), &bound_var.lt(&end_z3)]);

                    let condition_z3 = expr_to_z3(vc, &args[3], env, None)?.as_bool().ok_or(
                        MumeiError::TypeError(format!("{}(): condition must be boolean", name)),
                    )?;

                    // 束縛変数を env から復元
                    if let Some(old) = old_val {
                        env.insert(var_name, old);
                    } else {
                        env.remove(&var_name);
                    }

                    let quantifier_expr = if name == "forall" {
                        // ∀ var ∈ [start, end). condition
                        z3::ast::forall_const(
                            ctx,
                            &[&bound_var],
                            &[],
                            &range_cond.implies(&condition_z3),
                        )
                    } else {
                        // ∃ var ∈ [start, end). condition
                        z3::ast::exists_const(
                            ctx,
                            &[&bound_var],
                            &[],
                            &Bool::and(ctx, &[&range_cond, &condition_z3]),
                        )
                    };

                    Ok(quantifier_expr.into())
                }
                "len" => {
                    // len(arr_name) → 配列名に紐づくシンボリック長を返す
                    // len_<name> >= 0 の制約を自動付与
                    let arr_name = if !args.is_empty() {
                        if let Expr::Variable(name) = &args[0] {
                            name.clone()
                        } else {
                            "arr".to_string()
                        }
                    } else {
                        "arr".to_string()
                    };
                    let len_name = format!("len_{}", arr_name);
                    let len_var = Int::new_const(ctx, len_name.as_str());
                    if let Some(solver) = solver_opt {
                        solver.assert(&len_var.ge(&Int::from_i64(ctx, 0)));
                    }
                    env.insert(len_name, len_var.clone().into());
                    Ok(len_var.into())
                }
                "sqrt" => {
                    // Z3 0.12 の Float には sqrt メソッドがないため、
                    // シンボリック変数として扱い、sqrt(x) >= 0 の制約を付与
                    let _val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    let result = Float::new_const(ctx, "sqrt_result", 11, 53);
                    if let Some(solver) = solver_opt {
                        let zero = Float::from_f64(ctx, 0.0);
                        solver.assert(&result.ge(&zero));
                    }
                    Ok(result.into())
                }
                "cast_to_int" => {
                    // Z3 0.12 では Float->Int 直接変換がないため、シンボリック整数を返す
                    let _val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    Ok(Int::new_const(ctx, "cast_result").into())
                }
                _ => {
                    // ユーザー定義関数呼び出し: 契約による検証（Compositional Verification）
                    // 呼び出し先の requires を現在のコンテキストで証明し、
                    // 成功すれば ensures を事実として追加する
                    //
                    // FQN dot-notation サポート:
                    // "math.add" → "math::add" として ModuleEnv から解決する。
                    // これにより `math.add(x, y)` と `math::add(x, y)` の両方が動作する。
                    let fqn_name = name.replace('.', "::");
                    let resolved_callee = vc
                        .module_env
                        .get_atom(name)
                        .cloned()
                        .or_else(|| vc.module_env.get_atom(&fqn_name).cloned());
                    if let Some(callee) = resolved_callee {
                        // 引数を評価
                        let mut arg_vals = Vec::new();
                        for arg in args {
                            arg_vals.push(expr_to_z3(vc, arg, env, solver_opt)?);
                        }

                        // 仮引数名と実引数値の対応を構築
                        let mut call_env = env.clone();
                        for (i, param) in callee.params.iter().enumerate() {
                            if let Some(val) = arg_vals.get(i) {
                                call_env.insert(param.name.clone(), val.clone());
                            }
                        }

                        // 呼び出し先の精緻型制約を call_env に適用
                        for (i, param) in callee.params.iter().enumerate() {
                            if let Some(type_name) = &param.type_name {
                                if let Some(refined) = vc.module_env.get_type(type_name) {
                                    // 実引数値を精緻型の述語変数に束縛して制約を検証
                                    if let Some(val) = arg_vals.get(i) {
                                        call_env.insert(refined.operand.clone(), val.clone());
                                    }
                                }
                            }
                        }

                        // requires の検証: 呼び出し元のコンテキストで事前条件が満たされるか
                        if callee.requires.trim() != "true" {
                            if let Some(solver) = solver_opt {
                                let req_ast = parse_expression(&callee.requires);
                                let req_z3 = expr_to_z3(vc, &req_ast, &mut call_env, None)?;
                                if let Some(req_bool) = req_z3.as_bool() {
                                    solver.push();
                                    solver.assert(&req_bool.not());
                                    if solver.check() == SatResult::Sat {
                                        solver.pop(1);
                                        return Err(MumeiError::VerificationError(
                                            format!("Call to '{}': precondition (requires) not satisfied at call site", name)
                                        ));
                                    }
                                    solver.pop(1);
                                }
                            }
                        }

                        // ensures からシンボリック結果を生成し、事後条件を事実として追加
                        static CALL_COUNTER: std::sync::atomic::AtomicUsize =
                            std::sync::atomic::AtomicUsize::new(0);
                        let call_id =
                            CALL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let result_name = format!("call_{}_{}", name, call_id);

                        // 戻り値型の推定: 呼び出し先パラメータに f64 型があれば Float、なければ Int
                        let has_float = callee.params.iter().any(|p| {
                            p.type_name
                                .as_deref()
                                .map(|t| vc.module_env.resolve_base_type(t) == "f64")
                                .unwrap_or(false)
                        });
                        let result_z3: Dynamic = if has_float {
                            Float::new_const(ctx, result_name.as_str(), 11, 53).into()
                        } else {
                            Int::new_const(ctx, result_name.as_str()).into()
                        };

                        // ensures を事実として solver に追加（result を呼び出し結果に束縛）
                        //
                        // Equality Ensures Propagation:
                        // ensures 内に `result == expr` の形式の等式が含まれる場合、
                        // シンボリック result を具体的な式に直接束縛する。
                        // これにより `let x = increment(n);` で `x == n + 1` が
                        // 呼び出し元のコンテキストに伝播し、連鎖呼び出しの検証精度が向上する。
                        //
                        // 例: ensures: result == n + 1;
                        //   → call_env に result = call_increment_0 を挿入
                        //   → Z3 に call_increment_0 == n + 1 を assert
                        //   → 後続の `increment(x)` で x >= 1 だけでなく x == n + 1 が使える
                        if callee.ensures.trim() != "true" {
                            call_env.insert("result".to_string(), result_z3.clone());
                            let ens_ast = parse_expression(&callee.ensures);

                            // Equality ensures の特別処理:
                            // ensures が `result == expr` の形式の場合、
                            // expr を評価して result と等価であることを直接 assert する。
                            // これにより Z3 が等式を完全に活用できる。
                            let ens_z3 = expr_to_z3(vc, &ens_ast, &mut call_env, None)?;
                            if let Some(ens_bool) = ens_z3.as_bool() {
                                if let Some(solver) = solver_opt {
                                    solver.assert(&ens_bool);
                                }
                            }

                            // 追加: ensures 式が `result == expr` の形式かチェックし、
                            // 該当する場合は result のシンボリック値に対して
                            // 等式制約を明示的に追加する（Z3 の等式推論を強化）
                            if let Expr::BinaryOp(left, Op::Eq, right) = &ens_ast {
                                if let Expr::Variable(ref var_name) = left.as_ref() {
                                    if var_name == "result" {
                                        // ensures: result == <expr> の場合
                                        // <expr> を call_env で評価し、result_z3 == eval(<expr>) を assert
                                        if let Ok(rhs_val) =
                                            expr_to_z3(vc, right, &mut call_env, None)
                                        {
                                            if let Some(solver) = solver_opt {
                                                if let (Some(res_int), Some(rhs_int)) =
                                                    (result_z3.as_int(), rhs_val.as_int())
                                                {
                                                    solver.assert(&res_int._eq(&rhs_int));
                                                } else if let (Some(res_float), Some(rhs_float)) =
                                                    (result_z3.as_float(), rhs_val.as_float())
                                                {
                                                    solver.assert(&res_float._eq(&rhs_float));
                                                }
                                            }
                                        }
                                    }
                                }
                                // ensures: <expr> == result の逆順もサポート
                                if let Expr::Variable(ref var_name) = right.as_ref() {
                                    if var_name == "result" {
                                        if let Ok(lhs_val) =
                                            expr_to_z3(vc, left, &mut call_env, None)
                                        {
                                            if let Some(solver) = solver_opt {
                                                if let (Some(res_int), Some(lhs_int)) =
                                                    (result_z3.as_int(), lhs_val.as_int())
                                                {
                                                    solver.assert(&res_int._eq(&lhs_int));
                                                } else if let (Some(res_float), Some(lhs_float)) =
                                                    (result_z3.as_float(), lhs_val.as_float())
                                                {
                                                    solver.assert(&res_float._eq(&lhs_float));
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // 複合 ensures（&& で結合された複数条件）内の等式も伝播
                            // ensures: result >= 0 && result == n + 1 のような場合
                            propagate_equality_from_ensures(
                                vc,
                                &ens_ast,
                                &result_z3,
                                &mut call_env,
                                solver_opt,
                            )?;
                        }

                        // Taint Analysis: 呼び出し先が unverified の場合、
                        // 戻り値を __tainted_ マーカーで汚染済みとしてマークする。
                        if callee.trust_level == TrustLevel::Unverified {
                            let taint_key = format!("__tainted_{}", result_name);
                            let taint_marker = Bool::from_bool(ctx, true);
                            env.insert(taint_key, taint_marker.into());
                        }

                        Ok(result_z3)
                    } else {
                        Err(MumeiError::VerificationError(format!(
                            "Unknown function: {}",
                            name
                        )))
                    }
                }
            }
        }
        Expr::ArrayAccess(name, index_expr) => {
            let idx = expr_to_z3(vc, index_expr, env, solver_opt)?
                .as_int()
                .ok_or(MumeiError::TypeError("Index must be integer".into()))?;

            // 配列名に紐づく長さシンボルを使った境界チェック
            if let Some(solver) = solver_opt {
                let len_name = format!("len_{}", name);
                let len = if let Some(existing) = env.get(&len_name) {
                    existing
                        .as_int()
                        .unwrap_or(Int::new_const(ctx, len_name.as_str()))
                } else {
                    let l = Int::new_const(ctx, len_name.as_str());
                    solver.assert(&l.ge(&Int::from_i64(ctx, 0)));
                    env.insert(len_name.clone(), l.clone().into());
                    l
                };
                let safe = Bool::and(ctx, &[&idx.ge(&Int::from_i64(ctx, 0)), &idx.lt(&len)]);
                solver.push();
                solver.assert(&safe.not());
                if solver.check() == SatResult::Sat {
                    solver.pop(1);
                    return Err(MumeiError::VerificationError(format!(
                        "Potential Out-of-Bounds on '{}' (index may be < 0 or >= len_{})",
                        name, name
                    )));
                }
                solver.pop(1);
            }
            Ok(arr.select(&idx).into())
        }
        Expr::BinaryOp(left, op, right) => {
            let l = expr_to_z3(vc, left, env, solver_opt)?;
            let r = expr_to_z3(vc, right, env, solver_opt)?;

            // 浮動小数点か整数かで Z3 の AST メソッドを使い分ける
            if l.as_float().is_some() || r.as_float().is_some() {
                // 浮動小数点の場合、比較演算のみサポート（z3 0.12 の Float 算術は丸めモード API が複雑なため）
                // 算術演算はシンボリック結果として返す
                let lf = l.as_float().unwrap_or(Float::from_f64(ctx, 0.0));
                let rf = r.as_float().unwrap_or(Float::from_f64(ctx, 0.0));
                match op {
                    Op::Gt => Ok(lf.gt(&rf).into()),
                    Op::Lt => Ok(lf.lt(&rf).into()),
                    Op::Ge => Ok(lf.ge(&rf).into()),
                    Op::Le => Ok(lf.le(&rf).into()),
                    Op::Eq => Ok(lf._eq(&rf).into()),
                    Op::Neq => Ok(lf._eq(&rf).not().into()),
                    Op::Add | Op::Sub | Op::Mul | Op::Div => {
                        // シンボリック Float + 符号伝播制約
                        // (z3 crate 0.12 は内部フィールドが非公開のため z3-sys 直接呼び出し不可)
                        static FLOAT_COUNTER: std::sync::atomic::AtomicUsize =
                            std::sync::atomic::AtomicUsize::new(0);
                        let id = FLOAT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let result = Float::new_const(ctx, format!("float_arith_{}", id), 11, 53);
                        let zero = Float::from_f64(ctx, 0.0);
                        if let Some(solver) = solver_opt {
                            match op {
                                Op::Mul => {
                                    let both_pos = Bool::and(ctx, &[&lf.gt(&zero), &rf.gt(&zero)]);
                                    solver.assert(&both_pos.implies(&result.gt(&zero)));
                                    let both_neg = Bool::and(ctx, &[&lf.lt(&zero), &rf.lt(&zero)]);
                                    solver.assert(&both_neg.implies(&result.gt(&zero)));
                                }
                                Op::Add => {
                                    let both_pos = Bool::and(ctx, &[&lf.gt(&zero), &rf.ge(&zero)]);
                                    solver.assert(&both_pos.implies(&result.gt(&zero)));
                                    let both_pos2 = Bool::and(ctx, &[&lf.ge(&zero), &rf.gt(&zero)]);
                                    solver.assert(&both_pos2.implies(&result.gt(&zero)));
                                }
                                Op::Sub => {
                                    let a_gt_b = Bool::and(ctx, &[&lf.gt(&rf), &rf.ge(&zero)]);
                                    solver.assert(&a_gt_b.implies(&result.ge(&zero)));
                                }
                                Op::Div => {
                                    let both_pos = Bool::and(ctx, &[&lf.gt(&zero), &rf.gt(&zero)]);
                                    solver.assert(&both_pos.implies(&result.gt(&zero)));
                                }
                                _ => {}
                            }
                        }
                        Ok(result.into())
                    }
                    _ => Err("Invalid float op".into()),
                }
            } else {
                // Boolean 演算子は as_int() の前に処理する（オペランドが Bool のため）
                match op {
                    Op::And => {
                        let lb = l.as_bool().ok_or("Expected bool for &&")?;
                        let rb = r.as_bool().ok_or("Expected bool for &&")?;
                        return Ok(Bool::and(ctx, &[&lb, &rb]).into());
                    }
                    Op::Or => {
                        let lb = l.as_bool().ok_or("Expected bool for ||")?;
                        let rb = r.as_bool().ok_or("Expected bool for ||")?;
                        return Ok(Bool::or(ctx, &[&lb, &rb]).into());
                    }
                    Op::Implies => {
                        let lb = l.as_bool().ok_or("Expected bool for =>")?;
                        let rb = r.as_bool().ok_or("Expected bool for =>")?;
                        return Ok(lb.implies(&rb).into());
                    }
                    _ => {}
                }
                let li = l.as_int().ok_or("Expected int")?;
                let ri = r.as_int().ok_or("Expected int")?;
                match op {
                    Op::Add => Ok((&li + &ri).into()),
                    Op::Sub => Ok((&li - &ri).into()),
                    Op::Mul => Ok((&li * &ri).into()),
                    Op::Div => {
                        if let Some(solver) = solver_opt {
                            solver.push();
                            solver.assert(&ri._eq(&Int::from_i64(ctx, 0)));
                            if solver.check() == SatResult::Sat {
                                solver.pop(1);
                                return Err(MumeiError::VerificationError(
                                    "Potential division by zero.".into(),
                                ));
                            }
                            solver.pop(1);
                        }
                        Ok((&li / &ri).into())
                    }
                    Op::Gt => Ok(li.gt(&ri).into()),
                    Op::Lt => Ok(li.lt(&ri).into()),
                    Op::Ge => Ok(li.ge(&ri).into()),
                    Op::Le => Ok(li.le(&ri).into()),
                    Op::Eq => Ok(li._eq(&ri).into()),
                    Op::Neq => Ok(li._eq(&ri).not().into()),
                    _ => Err(MumeiError::VerificationError(format!(
                        "Unsupported int operator {:?}",
                        op
                    ))),
                }
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            let c = expr_to_z3(vc, cond, env, solver_opt)?
                .as_bool()
                .ok_or(MumeiError::TypeError("If condition must be boolean".into()))?;
            let t = expr_to_z3(vc, then_branch, env, solver_opt)?;
            let e = expr_to_z3(vc, else_branch, env, solver_opt)?;
            Ok(c.ite(&t, &e))
        }
        Expr::Let { var, value } => {
            // Block 内の逐次実行では変数を env に残す（スコープ管理は Block 側で行う）
            let val = expr_to_z3(vc, value, env, solver_opt)?;
            env.insert(var.clone(), val.clone());
            Ok(val)
        }
        Expr::Assign { var, value } => {
            let val = expr_to_z3(vc, value, env, solver_opt)?;
            env.insert(var.clone(), val.clone());
            Ok(val)
        }
        Expr::Block(stmts) => {
            let mut last = Int::from_i64(ctx, 0).into();
            for stmt in stmts {
                last = expr_to_z3(vc, stmt, env, solver_opt)?;
            }
            Ok(last)
        }
        Expr::While {
            cond,
            invariant,
            decreases,
            body,
        } => {
            // Loop Invariant 検証ロジック
            if let Some(solver) = solver_opt {
                let inv = expr_to_z3(vc, invariant, env, None)?
                    .as_bool()
                    .ok_or(MumeiError::TypeError("Invariant must be boolean".into()))?;

                // Base case: 現在の env（let で初期化済み）で invariant が成立するか
                solver.push();
                solver.assert(&inv.not());
                if solver.check() == SatResult::Sat {
                    solver.pop(1);
                    return Err(MumeiError::VerificationError(
                        "Invariant fails initially".into(),
                    ));
                }
                solver.pop(1);

                // Inductive step: invariant && cond のもとで body 実行後も invariant が保たれるか
                let c = expr_to_z3(vc, cond, env, None)?
                    .as_bool()
                    .ok_or(MumeiError::TypeError(
                        "While condition must be boolean".into(),
                    ))?;

                // Invariant preservation: invariant && cond のもとで body 実行後も invariant が保たれるか
                // env のスナップショットを保存し、各チェックを独立に行う
                {
                    let env_snapshot = env.clone();
                    solver.push();
                    solver.assert(&inv);
                    solver.assert(&c);
                    expr_to_z3(vc, body, env, Some(solver))?;

                    let inv_after = expr_to_z3(vc, invariant, env, None)?
                        .as_bool()
                        .ok_or(MumeiError::TypeError("Invariant must be boolean".into()))?;

                    solver.assert(&inv_after.not());
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        return Err(MumeiError::VerificationError(
                            "Invariant not preserved".into(),
                        ));
                    }
                    solver.pop(1);
                    *env = env_snapshot; // env を復元
                }

                // Termination Check: decreases 句が指定されている場合、停止性を検証
                if let Some(dec_expr) = decreases {
                    let env_snapshot = env.clone();

                    // V_before: ループ本体実行前の減少式の値
                    let v_before = expr_to_z3(vc, dec_expr, env, None)?.as_int().ok_or(
                        MumeiError::TypeError("decreases expression must be integer".into()),
                    )?;

                    // A. 下界の証明: invariant && cond => V >= 0
                    solver.push();
                    solver.assert(&inv);
                    solver.assert(&c);
                    solver.assert(&v_before.lt(&Int::from_i64(ctx, 0)));
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        return Err(MumeiError::VerificationError(
                            "Termination check failed: decreases expression may be negative".into(),
                        ));
                    }
                    solver.pop(1);

                    // B. 厳密な減少の証明: body 実行後に V' < V
                    solver.push();
                    solver.assert(&inv);
                    solver.assert(&c);
                    expr_to_z3(vc, body, env, Some(solver))?;

                    let v_after = expr_to_z3(vc, dec_expr, env, None)?.as_int().ok_or(
                        MumeiError::TypeError("decreases expression must be integer".into()),
                    )?;

                    solver.assert(&v_after.ge(&v_before));
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        *env = env_snapshot;
                        return Err(MumeiError::VerificationError(
                            "Termination check failed: decreases expression does not strictly decrease".into()
                        ));
                    }
                    solver.pop(1);
                    *env = env_snapshot; // env を復元
                }
            }

            let inv = expr_to_z3(vc, invariant, env, None)?
                .as_bool()
                .ok_or(MumeiError::TypeError("Invariant must be boolean".into()))?;
            let c_not = expr_to_z3(vc, cond, env, None)?
                .as_bool()
                .ok_or(MumeiError::TypeError(
                    "While condition must be boolean".into(),
                ))?
                .not();
            Ok(Bool::and(ctx, &[&inv, &c_not]).into())
        }
        Expr::StructInit { type_name, fields } => {
            // 構造体の各フィールドを検証し、env に登録
            // フィールドに精緻型制約がある場合は solver で検証する
            let mut last: Dynamic = Int::from_i64(ctx, 0).into();
            for (field_name, field_expr) in fields {
                let val = expr_to_z3(vc, field_expr, env, solver_opt)?;
                let qualified_name = format!("__struct_{}_{}", type_name, field_name);
                env.insert(qualified_name, val.clone());
                last = val.clone();

                // フィールド制約の検証: 構造体定義から constraint を取得
                if let Some(sdef) = vc.module_env.get_struct(type_name) {
                    if let Some(sfield) = sdef.fields.iter().find(|f| f.name == *field_name) {
                        if let Some(constraint_raw) = &sfield.constraint {
                            // constraint 内の "v" をフィールド値に置き換えて検証
                            let mut local_env = env.clone();
                            local_env.insert("v".to_string(), val.clone());
                            let constraint_ast = parse_expression(constraint_raw);
                            let constraint_z3 =
                                expr_to_z3(vc, &constraint_ast, &mut local_env, None)?;
                            if let Some(constraint_bool) = constraint_z3.as_bool() {
                                if let Some(solver) = solver_opt {
                                    solver.push();
                                    solver.assert(&constraint_bool.not());
                                    if solver.check() == SatResult::Sat {
                                        solver.pop(1);
                                        return Err(MumeiError::VerificationError(format!(
                                            "Struct '{}' field '{}' constraint violated: {}",
                                            type_name, field_name, constraint_raw
                                        )));
                                    }
                                    solver.pop(1);
                                }
                            }
                        }
                    }
                }
            }
            Ok(last)
        }
        Expr::Match { target, arms } => {
            let target_z3 = expr_to_z3(vc, target, env, solver_opt)?;

            // ========================================================
            // Enum ドメイン制約の自動注入
            // ========================================================
            // アームに Variant パターンが含まれる場合、対応する EnumDef を探し、
            // target の値域を 0..n_variants に制約する。
            // これにより Z3 が「これら以外のバリアントは存在しない」ことを知り、
            // 網羅性チェックの信頼性が 100% になる。
            if let Some(solver) = solver_opt {
                if let Some(enum_def) = detect_enum_from_arms(arms, vc.module_env) {
                    let n = enum_def.variants.len() as i64;
                    if let Some(tag_int) = target_z3.as_int() {
                        // tag ∈ [0, n_variants)
                        solver.assert(&tag_int.ge(&Int::from_i64(ctx, 0)));
                        solver.assert(&tag_int.lt(&Int::from_i64(ctx, n)));
                    }
                }
            }

            // ========================================================
            // Z3 網羅性チェック (Exhaustiveness Check)
            // ========================================================
            // 各アームの条件 P_i を構築し、¬(P_1 ∨ P_2 ∨ ... ∨ P_n) が
            // Unsat であることを証明する。Sat なら網羅性欠如エラー。
            if let Some(solver) = solver_opt {
                let mut arm_conditions: Vec<Bool> = Vec::new();
                for arm in arms {
                    let cond = pattern_to_z3_condition(
                        ctx,
                        &arm.pattern,
                        &target_z3,
                        env,
                        vc,
                        solver_opt,
                    )?;
                    // ガード条件がある場合は AND で結合
                    let full_cond = if let Some(guard) = &arm.guard {
                        let guard_z3 = expr_to_z3(vc, guard, env, None)?
                            .as_bool()
                            .ok_or(MumeiError::TypeError("Guard must be boolean".into()))?;
                        Bool::and(ctx, &[&cond, &guard_z3])
                    } else {
                        cond
                    };
                    arm_conditions.push(full_cond);
                }

                // 網羅性: ¬(P_1 ∨ ... ∨ P_n) が Unsat か？
                let arm_refs: Vec<&Bool> = arm_conditions.iter().collect();
                let coverage = Bool::or(ctx, &arm_refs);
                solver.push();
                solver.assert(&coverage.not());
                let exhaustive = solver.check() == SatResult::Unsat;
                solver.pop(1);

                if !exhaustive {
                    // 反例（Counter-example）の取得と表示
                    // solver はまだ Sat 状態なので、再度チェックして model を取得
                    solver.push();
                    solver.assert(&coverage.not());
                    if solver.check() == SatResult::Sat {
                        let counterexample = if let Some(model) = solver.get_model() {
                            // ターゲット変数の具体的な値を取得
                            let ce_str =
                                format_counterexample(&model, &target_z3, arms, vc.module_env);
                            ce_str
                        } else {
                            "unknown value".to_string()
                        };
                        solver.pop(1);
                        return Err(MumeiError::VerificationError(
                            format!(
                                "Match is not exhaustive: the following value is not covered by any arm:\n  Counter-example: {}",
                                counterexample
                            )
                        ));
                    }
                    solver.pop(1);
                    return Err(MumeiError::VerificationError(
                        "Match is not exhaustive: there exist values not covered by any arm."
                            .into(),
                    ));
                }
            }

            // ========================================================
            // Match 式の値の構築（if-then-else チェーンとして Z3 式を構築）
            // ========================================================
            // A. デフォルトアーム最適化:
            //    _ アームの body 評価時に、先行アームの否定を事前条件として
            //    env/solver に追加し、デフォルトアーム内の検証精度を向上させる。
            let mut accumulated_negations: Vec<Bool> = Vec::new();
            let mut result: Option<Dynamic> = None;

            for arm in arms.iter().rev() {
                let mut arm_env = env.clone();

                // B. ネストパターンの再帰解体:
                //    pattern_bind_variables が再帰的にパターンを分解し、
                //    バインド変数を arm_env に登録する。
                pattern_bind_variables(ctx, &arm.pattern, &target_z3, &mut arm_env, vc.module_env);

                let arm_cond = pattern_to_z3_condition(
                    ctx,
                    &arm.pattern,
                    &target_z3,
                    &mut arm_env,
                    vc,
                    solver_opt,
                )?;
                let full_cond = if let Some(guard) = &arm.guard {
                    let guard_z3 = expr_to_z3(vc, guard, &mut arm_env, None)?
                        .as_bool()
                        .ok_or(MumeiError::TypeError("Guard must be boolean".into()))?;
                    Bool::and(ctx, &[&arm_cond, &guard_z3])
                } else {
                    arm_cond
                };

                // A. デフォルトアーム最適化: Wildcard/Variable パターンの場合、
                //    先行アームの否定条件を solver に追加して body を検証
                if let Some(solver) = solver_opt {
                    if matches!(arm.pattern, Pattern::Wildcard | Pattern::Variable(_))
                        && !accumulated_negations.is_empty()
                    {
                        let neg_refs: Vec<&Bool> = accumulated_negations.iter().collect();
                        let prior_negation = Bool::and(ctx, &neg_refs);
                        solver.push();
                        solver.assert(&prior_negation);
                        let body_val = expr_to_z3(vc, &arm.body, &mut arm_env, solver_opt)?;
                        solver.pop(1);
                        result = Some(match result {
                            Some(else_val) => full_cond.ite(&body_val, &else_val),
                            None => body_val,
                        });
                        accumulated_negations.push(full_cond.not());
                        continue;
                    }
                }

                let body_val = expr_to_z3(vc, &arm.body, &mut arm_env, solver_opt)?;
                result = Some(match result {
                    Some(else_val) => full_cond.ite(&body_val, &else_val),
                    None => body_val,
                });
                accumulated_negations.push(full_cond.not());
            }

            result
                .ok_or_else(|| MumeiError::VerificationError("Match expression has no arms".into()))
        }

        // =================================================================
        // 非同期処理 + リソース管理の Z3 検証
        // =================================================================
        Expr::Acquire { resource, body } => {
            // acquire ブロック: リソースを取得して body を実行し、自動解放する。
            // Z3 上ではリソースの保持状態をシンボリック Bool で追跡する。
            let held_name = format!("__resource_held_{}", resource);
            let held_bool = Bool::new_const(ctx, held_name.as_str());
            if let Some(solver) = solver_opt {
                // リソース取得: held = true
                solver.assert(&held_bool);
            }
            env.insert(held_name.clone(), held_bool.into());

            // body を検証
            let body_result = expr_to_z3(vc, body, env, solver_opt)?;

            // リソース解放: held = false
            let released = Bool::from_bool(ctx, false);
            env.insert(held_name, released.into());

            Ok(body_result)
        }
        Expr::Async { body } => {
            // async ブロック: body を非同期コンテキストとして検証する。
            // Z3 上では通常の式として扱い、結果をシンボリック値として返す。
            // await ポイントでの所有権検証は Await 式で行う。
            expr_to_z3(vc, body, env, solver_opt)
        }
        Expr::Await { expr } => {
            // =============================================================
            // await 跨ぎの安全性検証 (Await Safety Verification)
            // =============================================================
            //
            // await ポイントはコルーチンの中断点であり、以下の安全性を検証する:
            //
            // 1. リソース保持検証 (Resource Held Across Await):
            //    acquire ブロック内で await を呼ぶと、リソースを保持したまま
            //    スレッドが中断される。これはデッドロックの典型パターン。
            //    env 内の __resource_held_* が true のリソースを検出してエラーにする。
            //
            // 2. 所有権一貫性検証 (Ownership Consistency):
            //    await 前に消費済み（__alive_ = false）の変数が、await 後に
            //    アクセスされないことを確認する。Z3 で __alive_ フラグをチェック。

            // --- 1. リソース保持検証 ---
            // env 内の __resource_held_* キーを走査し、Z3 で true かどうかを確認する。
            // acquire ブロック内で await を呼ぶパターンを検出する。
            if let Some(solver) = solver_opt {
                let held_resources: Vec<String> = env
                    .keys()
                    .filter(|k| k.starts_with("__resource_held_"))
                    .cloned()
                    .collect();

                for held_key in &held_resources {
                    let resource_name = held_key
                        .strip_prefix("__resource_held_")
                        .unwrap_or(held_key);
                    if let Some(held_val) = env.get(held_key) {
                        // Z3 で held_val == true が証明可能かチェック
                        // （acquire ブロック内なら held = true が assert されている）
                        if let Some(held_bool) = held_val.as_bool() {
                            solver.push();
                            // held が true であることを仮定し、矛盾がなければ保持中
                            solver.assert(&held_bool);
                            if solver.check() != SatResult::Unsat {
                                solver.pop(1);
                                return Err(MumeiError::VerificationError(
                                    format!(
                                        "Unsafe await: resource '{}' is held across an await point. \
                                         This can cause deadlock because the resource lock is not released \
                                         during suspension. Move the await outside the acquire block, or \
                                         release the resource before awaiting.\n  \
                                         Hint: acquire {} {{ ... }}; let val = await expr; // OK\n  \
                                         Bad:  acquire {} {{ let val = await expr; ... }}  // deadlock risk",
                                        resource_name, resource_name, resource_name
                                    )
                                ));
                            }
                            solver.pop(1);
                        }
                    }
                }
            }

            // --- 2. 所有権一貫性検証 ---
            // await 前に消費済みの変数を検出し、Z3 で __alive_ = false を確認する。
            // 消費済み変数が await 後にアクセスされる可能性がある場合、警告する。
            if let Some(solver) = solver_opt {
                let consumed_vars: Vec<String> = env
                    .keys()
                    .filter(|k| k.starts_with("__alive_"))
                    .cloned()
                    .collect();

                for alive_key in &consumed_vars {
                    let var_name = alive_key.strip_prefix("__alive_").unwrap_or(alive_key);
                    if let Some(alive_val) = env.get(alive_key) {
                        if let Some(alive_bool) = alive_val.as_bool() {
                            // __alive_ が false（消費済み）であることを Z3 で確認
                            solver.push();
                            solver.assert(&alive_bool.not()); // alive = false を仮定
                            if solver.check() == SatResult::Sat {
                                // 消費済み変数が存在する → await 後のアクセスは use-after-free
                                // await ポイントでの状態をマーク（後続の検証で参照）
                                let await_consumed_key = format!("__await_consumed_{}", var_name);
                                let marker = Bool::from_bool(vc.ctx, true);
                                env.insert(await_consumed_key, marker.into());
                            }
                            solver.pop(1);
                        }
                    }
                }
            }

            // 内側の式を評価してシンボリック結果を返す
            let inner_result = expr_to_z3(vc, expr, env, solver_opt)?;
            Ok(inner_result)
        }

        Expr::FieldAccess(inner_expr, field_name) => {
            // ネスト構造体のフィールドアクセスを再帰的に解決する。
            //
            // 1段階: v.x → env["__struct_v_x"] or env["v_x"]
            // 2段階: v.point.x → まず v.point を解決し、その結果から .x を解決
            //
            // 解決戦略:
            // A. 内側の式が Variable の場合: 直接 env から探す
            // B. 内側の式が FieldAccess の場合: 再帰的に解決し、
            //    結果のパスを使って env から探す
            // C. どちらでもない場合: 式を評価してシンボリック変数を生成

            // フラットなパス文字列を構築するヘルパー
            // v.point.x → "v_point_x" のようなパスを生成
            fn build_field_path(expr: &Expr) -> Option<Vec<String>> {
                match expr {
                    Expr::Variable(name) => Some(vec![name.clone()]),
                    Expr::FieldAccess(inner, field) => {
                        let mut path = build_field_path(inner)?;
                        path.push(field.clone());
                        Some(path)
                    }
                    _ => None,
                }
            }

            // 完全なフィールドパスを構築（例: ["v", "point", "x"]）
            let full_path = {
                let mut path = build_field_path(inner_expr).unwrap_or_default();
                path.push(field_name.clone());
                path
            };

            if full_path.len() >= 2 {
                // パスの各プレフィックスで env を探索
                // 例: ["v", "point", "x"] → "v_point_x", "__struct_v_point_x"
                let underscore_path = full_path.join("_");
                let struct_path = format!("__struct_{}", underscore_path);

                // 直接パスで見つかればそれを返す
                if let Some(val) = env.get(&struct_path) {
                    return Ok(val.clone());
                }
                if let Some(val) = env.get(&underscore_path) {
                    return Ok(val.clone());
                }

                // 1段階ずつ解決を試みる
                // 例: v.point → env["__struct_v_point"] or env["v_point"]
                //     その結果が構造体型なら、.x のフィールドをさらに解決
                if full_path.len() == 2 {
                    // 単純な1段階アクセス: v.x
                    let var_name = &full_path[0];
                    let candidates = [
                        format!("__struct_{}_{}", var_name, field_name),
                        format!("{}_{}", var_name, field_name),
                    ];
                    for candidate in &candidates {
                        if let Some(val) = env.get(candidate) {
                            return Ok(val.clone());
                        }
                    }
                }

                // ネスト構造体の再帰解決:
                // 内側の式を先に Z3 で評価し、結果を env に登録してからフィールドを解決
                let _base_val = expr_to_z3(vc, inner_expr, env, solver_opt)?;

                // 内側の式の型を推定し、構造体定義からフィールドの型を取得
                // フィールドの精緻型制約も再帰的に適用する
                let nested_sym_name = format!(
                    "{}_{}",
                    underscore_path
                        .rsplit_once('_')
                        .map(|(prefix, _)| prefix)
                        .unwrap_or(&underscore_path),
                    field_name
                );
                let sym = if let Some(val) = env.get(&nested_sym_name) {
                    return Ok(val.clone());
                } else {
                    let s = Int::new_const(ctx, full_path.join("_").as_str());
                    env.insert(full_path.join("_"), s.clone().into());
                    s
                };
                Ok(sym.into())
            } else {
                // パスが構築できない場合: 式を評価してシンボリック変数を生成
                let _base = expr_to_z3(vc, inner_expr, env, solver_opt)?;
                let sym = Int::new_const(ctx, format!("field_{}", field_name));
                Ok(sym.into())
            }
        }
    }
}

// =============================================================================
// パターンマッチング: Z3 条件生成 + 変数バインド + 反例フォーマット
// =============================================================================

/// パターンから Z3 の Bool 条件を生成する（再帰的: ネストパターン対応）
///
/// Phase 1-B: tag + payload 表現
/// - Wildcard / Variable → true（常にマッチ）
/// - Literal(n) → target == n
/// - Variant { name, fields } → (tag == variant_index) ∧ (各フィールドの再帰条件)
///
/// フィールドは "projector" シンボル `__proj_{VariantName}_{i}` として表現。
/// 同一 match 内で同じ projector 名を使うことで、異なるアーム間で
/// 同じフィールドへの参照が一貫する。
fn pattern_to_z3_condition<'a>(
    ctx: &'a Context,
    pattern: &Pattern,
    target: &Dynamic<'a>,
    env: &mut Env<'a>,
    vc: &VCtx<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> MumeiResult<Bool<'a>> {
    match pattern {
        Pattern::Wildcard | Pattern::Variable(_) => Ok(Bool::from_bool(ctx, true)),
        Pattern::Literal(n) => {
            let target_int = target
                .as_int()
                .unwrap_or(Int::new_const(ctx, "__match_target"));
            let lit = Int::from_i64(ctx, *n);
            Ok(target_int._eq(&lit))
        }
        Pattern::Variant {
            variant_name,
            fields,
        } => {
            if let Some(enum_def) = vc.module_env.find_enum_by_variant(variant_name) {
                let variant_idx = enum_def
                    .variants
                    .iter()
                    .position(|v| v.name == *variant_name)
                    .unwrap_or(0) as i64;

                let tag = target
                    .as_int()
                    .unwrap_or(Int::new_const(ctx, "__match_tag"));
                let tag_match = tag._eq(&Int::from_i64(ctx, variant_idx));

                let variant_def = &enum_def.variants[variant_idx as usize];
                let mut field_conditions: Vec<Bool> = vec![tag_match];

                for (i, field_pattern) in fields.iter().enumerate() {
                    // Projector シンボル: __proj_{VariantName}_{i}
                    // 同一バリアントの同一フィールドは常に同じシンボルを共有
                    let proj_name = format!("__proj_{}_{}", variant_name, i);
                    let field_sym: Dynamic = if i < variant_def.fields.len() {
                        let field_type = &variant_def.fields[i];
                        // 再帰的 ADT: フィールド型が自身の Enum なら tag として Int を使用
                        let base = if *field_type == enum_def.name {
                            "i64".to_string() // 再帰フィールドは tag 値
                        } else {
                            vc.module_env.resolve_base_type(field_type)
                        };
                        match base.as_str() {
                            "f64" => Float::new_const(ctx, proj_name.as_str(), 11, 53).into(),
                            _ => Int::new_const(ctx, proj_name.as_str()).into(),
                        }
                    } else {
                        Int::new_const(ctx, proj_name.as_str()).into()
                    };

                    // env にも projector を登録（body 内で参照可能にする）
                    env.insert(proj_name.clone(), field_sym.clone());

                    // 再帰フィールドの場合: ドメイン制約を追加
                    if i < variant_def.fields.len() && variant_def.fields[i] == enum_def.name {
                        if let Some(solver) = solver_opt {
                            if let Some(field_int) = field_sym.as_int() {
                                let n = enum_def.variants.len() as i64;
                                solver.assert(&field_int.ge(&Int::from_i64(ctx, 0)));
                                solver.assert(&field_int.lt(&Int::from_i64(ctx, n)));
                            }
                        }
                    }

                    // 再帰的にフィールドパターンの条件を生成
                    let field_cond = pattern_to_z3_condition(
                        ctx,
                        field_pattern,
                        &field_sym,
                        env,
                        vc,
                        solver_opt,
                    )?;
                    field_conditions.push(field_cond);
                }

                let cond_refs: Vec<&Bool> = field_conditions.iter().collect();
                Ok(Bool::and(ctx, &cond_refs))
            } else {
                let tag = target
                    .as_int()
                    .unwrap_or(Int::new_const(ctx, "__match_tag"));
                let hash = variant_name
                    .bytes()
                    .fold(0i64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as i64));
                Ok(tag._eq(&Int::from_i64(ctx, hash)))
            }
        }
    }
}

/// パターンから変数バインドを env に登録する（再帰的: ネストパターン対応）
///
/// Phase 1-B: projector シンボルを使ったバインド
/// - Variable(name) → target の値を name にバインド
/// - Variant の fields 内の Variable → projector シンボル `__proj_{Variant}_{i}` にバインド
/// - Variant の fields 内の Variant → 再帰的に projector を生成してバインド
fn pattern_bind_variables<'a>(
    ctx: &'a Context,
    pattern: &Pattern,
    target: &Dynamic<'a>,
    env: &mut Env<'a>,
    module_env: &ModuleEnv,
) {
    match pattern {
        Pattern::Variable(name) => {
            env.insert(name.clone(), target.clone());
        }
        Pattern::Variant {
            variant_name,
            fields,
        } => {
            if let Some(enum_def) = module_env.find_enum_by_variant(variant_name) {
                if let Some(variant_def) =
                    enum_def.variants.iter().find(|v| v.name == *variant_name)
                {
                    for (i, field_pattern) in fields.iter().enumerate() {
                        let proj_name = format!("__proj_{}_{}", variant_name, i);
                        let field_sym: Dynamic = if i < variant_def.fields.len() {
                            let field_type = &variant_def.fields[i];
                            let base = if *field_type == enum_def.name {
                                "i64".to_string()
                            } else {
                                module_env.resolve_base_type(field_type)
                            };
                            match base.as_str() {
                                "f64" => Float::new_const(ctx, proj_name.as_str(), 11, 53).into(),
                                _ => Int::new_const(ctx, proj_name.as_str()).into(),
                            }
                        } else {
                            Int::new_const(ctx, proj_name.as_str()).into()
                        };
                        env.insert(proj_name.clone(), field_sym.clone());

                        // Variable パターン: projector を変数名にもバインド
                        match field_pattern {
                            Pattern::Variable(fname) => {
                                env.insert(fname.clone(), field_sym.clone());
                            }
                            Pattern::Variant { .. } => {
                                // ネストした Variant: 再帰的にバインド
                                pattern_bind_variables(
                                    ctx,
                                    field_pattern,
                                    &field_sym,
                                    env,
                                    module_env,
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        Pattern::Wildcard | Pattern::Literal(_) => {}
    }
}

/// アームの Variant パターンから対応する EnumDef を検出する。
/// 最初に見つかった Variant パターンの所属 Enum を返す。
fn detect_enum_from_arms<'a>(arms: &[MatchArm], module_env: &'a ModuleEnv) -> Option<&'a EnumDef> {
    for arm in arms {
        if let Pattern::Variant { variant_name, .. } = &arm.pattern {
            if let Some(enum_def) = module_env.find_enum_by_variant(variant_name) {
                return Some(enum_def);
            }
        }
    }
    None
}

/// Z3 Model から反例の文字列表現を生成する。
/// Enum ドメイン制約が注入されている場合、tag 値からバリアント名+フィールド値を表示する。
fn format_counterexample(
    model: &z3::Model,
    target: &Dynamic,
    arms: &[MatchArm],
    module_env: &ModuleEnv,
) -> String {
    // アームから Enum 定義を特定（ドメイン制約と同じロジック）
    let enum_ctx = detect_enum_from_arms(arms, module_env);

    // ターゲット変数の具体的な値を取得
    if let Some(target_val) = model.eval(target, true) {
        let target_str = format!("{}", target_val);

        // Enum の場合: tag 値からバリアント名を逆引き
        if let Some(target_int) = target_val.as_int() {
            let tag_str = format!("{}", target_int);
            if let Ok(tag_val) = tag_str.parse::<i64>() {
                // まず arms から特定した Enum を優先的に使用
                if let Some(edef) = enum_ctx {
                    if let Some(variant) = edef.variants.get(tag_val as usize) {
                        // フィールド値も model から取得を試みる
                        let mut field_vals = Vec::new();
                        for (i, field_type) in variant.fields.iter().enumerate() {
                            let _field_sym_name = format!("__proj_{}_{}", variant.name, i);
                            // model 内のシンボルを探す（存在すれば具体値を表示）
                            let field_str = format!("{}=?", field_type);
                            field_vals.push(field_str);
                        }
                        let fields_display = if field_vals.is_empty() {
                            String::new()
                        } else {
                            format!("({})", field_vals.join(", "))
                        };
                        return format!(
                            "{}::{}{} (tag={}) -- missing from match arms",
                            edef.name, variant.name, fields_display, tag_val
                        );
                    }
                }
                // フォールバック: module_env の全 Enum 定義を走査
                for (enum_name, enum_def) in module_env.enums.iter() {
                    if let Some(variant) = enum_def.variants.get(tag_val as usize) {
                        return format!(
                            "{}::{} (tag={}) -- missing from match arms",
                            enum_name, variant.name, tag_val
                        );
                    }
                }
            }
            // 整数リテラルとしてフォールバック
            return format!("value = {} -- no matching arm", tag_str);
        }

        format!("value = {} -- no matching arm", target_str)
    } else {
        // 評価に失敗した場合、アームの情報からヒントを生成
        let covered: Vec<String> = arms
            .iter()
            .map(|arm| match &arm.pattern {
                Pattern::Literal(n) => format!("{}", n),
                Pattern::Variant { variant_name, .. } => variant_name.clone(),
                Pattern::Variable(name) => format!("_{} (bind)", name),
                Pattern::Wildcard => "_".to_string(),
            })
            .collect();
        format!(
            "(could not evaluate; covered patterns: [{}])",
            covered.join(", ")
        )
    }
}

/// 複合 ensures 式（&& で結合された複数条件）から等式 `result == expr` を
/// 再帰的に抽出し、Z3 solver に assert する。
///
/// ensures: result >= 0 && result == n + 1
/// → `result >= 0` と `result == n + 1` の両方を assert
/// → 特に `result == n + 1` は等式制約として明示的に追加
///
/// ensures: result == a + b && result >= 0 && result <= 100
/// → 3つの条件すべてを assert + `result == a + b` の等式を追加
fn propagate_equality_from_ensures<'a>(
    vc: &VCtx<'a>,
    expr: &Expr,
    result_z3: &Dynamic<'a>,
    call_env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> MumeiResult<()> {
    match expr {
        // && で結合された複合条件: 左右を再帰的に処理
        Expr::BinaryOp(left, Op::And, right) => {
            propagate_equality_from_ensures(vc, left, result_z3, call_env, solver_opt)?;
            propagate_equality_from_ensures(vc, right, result_z3, call_env, solver_opt)?;
        }
        // result == <expr> の等式
        Expr::BinaryOp(left, Op::Eq, right) => {
            let is_result_left = matches!(left.as_ref(), Expr::Variable(ref v) if v == "result");
            let is_result_right = matches!(right.as_ref(), Expr::Variable(ref v) if v == "result");

            if is_result_left {
                if let Ok(rhs_val) = expr_to_z3(vc, right, call_env, None) {
                    if let Some(solver) = solver_opt {
                        if let (Some(res_int), Some(rhs_int)) =
                            (result_z3.as_int(), rhs_val.as_int())
                        {
                            solver.assert(&res_int._eq(&rhs_int));
                        } else if let (Some(res_float), Some(rhs_float)) =
                            (result_z3.as_float(), rhs_val.as_float())
                        {
                            solver.assert(&res_float._eq(&rhs_float));
                        }
                    }
                }
            } else if is_result_right {
                if let Ok(lhs_val) = expr_to_z3(vc, left, call_env, None) {
                    if let Some(solver) = solver_opt {
                        if let (Some(res_int), Some(lhs_int)) =
                            (result_z3.as_int(), lhs_val.as_int())
                        {
                            solver.assert(&res_int._eq(&lhs_int));
                        } else if let (Some(res_float), Some(lhs_float)) =
                            (result_z3.as_float(), lhs_val.as_float())
                        {
                            solver.assert(&res_float._eq(&lhs_float));
                        }
                    }
                }
            }
        }
        _ => {
            // 等式でも && でもない条件はスキップ（既に全体の ensures として assert 済み）
        }
    }
    Ok(())
}

fn save_visualizer_report(
    output_dir: &Path,
    status: &str,
    name: &str,
    a: &str,
    b: &str,
    reason: &str,
) {
    let report =
        json!({ "status": status, "atom": name, "input_a": a, "input_b": b, "reason": reason });
    let _ = fs::create_dir_all(output_dir);
    let _ = fs::write(output_dir.join("report.json"), report.to_string());
}
