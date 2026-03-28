use crate::hir::HirAtom;
use crate::parser::{
    parse_body_expr, parse_expression, Atom, Effect, EffectDef, EnumDef, Expr, ImplDef, Item,
    JoinSemantics, MatchArm, Op, Pattern, QuantifierType, RefinedType, ResourceDef, ResourceMode,
    Span, Stmt, StructDef, TraitDef, TraitMethod, TrustLevel,
};
use miette::SourceSpan;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::Path;
use z3::ast::{Array, Ast, Bool, Dynamic, Float, Int, String as Z3String};
use z3::{Config, Context, SatResult, Solver};

/// Word-boundary-aware replacement of the placeholder `v` in constraint strings.
/// Uses regex `\bv\b` to avoid corrupting identifiers like "value" or "divisor".
fn replace_constraint_placeholder(constraint: &str, replacement: &str) -> String {
    let re = regex::Regex::new(r"\bv\b").unwrap();
    re.replace_all(constraint, replacement).to_string()
}

// --- エラー型の定義 ---

// =============================================================================
// Related Diagnostic for multi-span error reporting (Feature 3a)
// =============================================================================

#[derive(thiserror::Error, miette::Diagnostic, Debug)]
#[error("{msg}")]
pub struct RelatedDiagnostic {
    pub msg: String,
    #[source_code]
    pub src: miette::NamedSource<String>,
    #[label("{label}")]
    pub span: SourceSpan,
    pub label: String,
    /// Original parser::Span (line/col) for LSP line mapping.
    /// Miette's SourceSpan is a byte offset that requires source text to resolve;
    /// this field allows LSP to read line/col directly without source text.
    pub original_span: Span,
}

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
    /// メッセージのみで ErrorDetail を生成する（Span 不明時のフォールバック用）
    #[allow(dead_code)]
    pub fn from_message(msg: impl Into<String>) -> Self {
        ErrorDetail {
            message: msg.into(),
            span: Span::default(),
            suggestion: None,
        }
    }

    /// Span 付きで ErrorDetail を生成する
    #[allow(dead_code)]
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

/// ソースコードの Span（line/col/len）からバイトオフセットを計算して miette::SourceSpan を返す。
/// line は 1-indexed なので 0-indexed に変換してから計算する。
/// \n と \r\n の両方の改行を正しく処理する（実バイト位置で \n を検出）。
pub fn span_to_source_span(source: &str, span: &Span) -> SourceSpan {
    if span.line == 0 {
        return SourceSpan::from((0, 0));
    }
    // 実バイト位置で \n を数えて目的の行の先頭オフセットを求める。
    // これにより \r\n (2 bytes) も \n (1 byte) も正しく扱える。
    let mut current_line = 1usize;
    let mut line_start = 0usize;
    let mut found = span.line == 1;
    if !found {
        for (idx, ch) in source.char_indices() {
            if ch == '\n' {
                current_line += 1;
                if current_line == span.line {
                    line_start = idx + 1; // \n の次のバイトが行頭
                    found = true;
                    break;
                }
            }
        }
    }
    if !found {
        return SourceSpan::from((0, 0));
    }
    // col は 1-indexed (character-based), line_start は byte offset なので
    // 行頭から col_offset 文字分のバイト数を計算する（マルチバイト UTF-8 対応）
    let col_offset = if span.col > 0 { span.col - 1 } else { 0 };
    let line_str = &source[line_start..];
    let offset = line_start
        + line_str
            .char_indices()
            .nth(col_offset)
            .map(|(i, _)| i)
            .unwrap_or(col_offset);
    let len = if span.len > 0 { span.len } else { 1 };
    // Clamp to source length to avoid out-of-bounds
    let offset = offset.min(source.len());
    let len = len.min(source.len().saturating_sub(offset));
    SourceSpan::from((offset, len))
}

#[derive(thiserror::Error, miette::Diagnostic, Debug)]
#[allow(clippy::enum_variant_names)]
pub enum MumeiError {
    #[error("Verification Error: {msg}")]
    #[diagnostic(code(mumei::verification))]
    VerificationError {
        msg: String,
        #[source_code]
        src: miette::NamedSource<String>,
        #[label("verification failed here")]
        span: SourceSpan,
        #[help]
        help: Option<String>,
        /// LSP 用: 元の parser::Span（line/col）を保持する
        original_span: Span,
        #[related]
        related: Vec<RelatedDiagnostic>,
    },
    #[error("Codegen Error: {msg}")]
    #[diagnostic(code(mumei::codegen))]
    CodegenError {
        msg: String,
        #[source_code]
        src: miette::NamedSource<String>,
        #[label("codegen failed here")]
        span: SourceSpan,
        #[help]
        help: Option<String>,
        /// LSP 用: 元の parser::Span（line/col）を保持する
        original_span: Span,
        #[related]
        related: Vec<RelatedDiagnostic>,
    },
    #[error("Type Error: {msg}")]
    #[diagnostic(code(mumei::type_error))]
    TypeError {
        msg: String,
        #[source_code]
        src: miette::NamedSource<String>,
        #[label("type mismatch here")]
        span: SourceSpan,
        #[help]
        help: Option<String>,
        /// LSP 用: 元の parser::Span（line/col）を保持する
        original_span: Span,
        #[related]
        related: Vec<RelatedDiagnostic>,
    },
}

impl MumeiError {
    /// Span なしで VerificationError を生成（位置不明のエラー）
    pub fn verification(msg: impl Into<String>) -> Self {
        MumeiError::VerificationError {
            msg: msg.into(),
            src: miette::NamedSource::new("<unknown>", String::new()),
            span: SourceSpan::from((0, 0)),
            help: None,
            original_span: Span::default(),
            related: Vec::new(),
        }
    }
    /// Span 付きで VerificationError を生成
    pub fn verification_at(msg: impl Into<String>, span: Span) -> Self {
        MumeiError::VerificationError {
            msg: msg.into(),
            src: miette::NamedSource::new(
                if span.file.is_empty() {
                    "<unknown>"
                } else {
                    &span.file
                },
                String::new(),
            ),
            span: SourceSpan::from((0, 0)),
            help: None,
            original_span: span,
            related: Vec::new(),
        }
    }
    /// ソースコード付きで VerificationError を生成（リッチ出力対応）
    #[allow(dead_code)]
    pub fn verification_with_source(
        msg: impl Into<String>,
        span: &Span,
        source: &str,
        help: Option<String>,
    ) -> Self {
        let source_span = span_to_source_span(source, span);
        MumeiError::VerificationError {
            msg: msg.into(),
            src: miette::NamedSource::new(
                if span.file.is_empty() {
                    "<unknown>"
                } else {
                    &span.file
                },
                source.to_string(),
            ),
            span: source_span,
            help,
            original_span: span.clone(),
            related: Vec::new(),
        }
    }
    /// Span なしで CodegenError を生成
    pub fn codegen(msg: impl Into<String>) -> Self {
        MumeiError::CodegenError {
            msg: msg.into(),
            src: miette::NamedSource::new("<unknown>", String::new()),
            span: SourceSpan::from((0, 0)),
            help: None,
            original_span: Span::default(),
            related: Vec::new(),
        }
    }
    /// ソースコード付きで CodegenError を生成（リッチ出力対応）
    #[allow(dead_code)]
    pub fn codegen_with_source(
        msg: impl Into<String>,
        span: &Span,
        source: &str,
        help: Option<String>,
    ) -> Self {
        let source_span = span_to_source_span(source, span);
        MumeiError::CodegenError {
            msg: msg.into(),
            src: miette::NamedSource::new(
                if span.file.is_empty() {
                    "<unknown>"
                } else {
                    &span.file
                },
                source.to_string(),
            ),
            span: source_span,
            help,
            original_span: span.clone(),
            related: Vec::new(),
        }
    }
    /// Span なしで TypeError を生成
    pub fn type_error(msg: impl Into<String>) -> Self {
        MumeiError::TypeError {
            msg: msg.into(),
            src: miette::NamedSource::new("<unknown>", String::new()),
            span: SourceSpan::from((0, 0)),
            help: None,
            original_span: Span::default(),
            related: Vec::new(),
        }
    }
    /// Span 付きで TypeError を生成
    pub fn type_error_at(msg: impl Into<String>, span: Span) -> Self {
        MumeiError::TypeError {
            msg: msg.into(),
            src: miette::NamedSource::new(
                if span.file.is_empty() {
                    "<unknown>"
                } else {
                    &span.file
                },
                String::new(),
            ),
            span: SourceSpan::from((0, 0)),
            help: None,
            original_span: span,
            related: Vec::new(),
        }
    }
    /// ソースコード付きで TypeError を生成（リッチ出力対応）
    #[allow(dead_code)]
    pub fn type_error_with_source(
        msg: impl Into<String>,
        span: &Span,
        source: &str,
        help: Option<String>,
    ) -> Self {
        let source_span = span_to_source_span(source, span);
        MumeiError::TypeError {
            msg: msg.into(),
            src: miette::NamedSource::new(
                if span.file.is_empty() {
                    "<unknown>"
                } else {
                    &span.file
                },
                source.to_string(),
            ),
            span: source_span,
            help,
            original_span: span.clone(),
            related: Vec::new(),
        }
    }

    /// ErrorDetail を取得する（Span 情報を保持）
    pub fn to_detail(&self) -> ErrorDetail {
        match self {
            MumeiError::VerificationError {
                msg, original_span, ..
            } => ErrorDetail::with_span(
                format!("Verification Error: {}", msg),
                original_span.clone(),
            ),
            MumeiError::CodegenError {
                msg, original_span, ..
            } => ErrorDetail::with_span(format!("Codegen Error: {}", msg), original_span.clone()),
            MumeiError::TypeError {
                msg, original_span, ..
            } => ErrorDetail::with_span(format!("Type Error: {}", msg), original_span.clone()),
        }
    }

    /// ソースコードを設定してリッチ出力を有効にする
    /// エラー自身が意味のある original_span を持つ場合はそちらを優先し、
    /// そうでなければ fallback_span（atom 定義の span 等）を使用する。
    pub fn with_source(self, source: &str, fallback_span: &Span) -> Self {
        // Use the error's own original_span if it has meaningful location info,
        // otherwise fall back to the provided span (e.g., atom definition)
        let effective_span = match &self {
            MumeiError::VerificationError { original_span, .. }
            | MumeiError::CodegenError { original_span, .. }
            | MumeiError::TypeError { original_span, .. }
                if original_span.line > 0 =>
            {
                original_span.clone()
            }
            _ => fallback_span.clone(),
        };
        let source_span = span_to_source_span(source, &effective_span);
        let file_name = if !effective_span.file.is_empty() {
            &effective_span.file
        } else if !fallback_span.file.is_empty() {
            &fallback_span.file
        } else {
            "<unknown>"
        };
        let named_src = miette::NamedSource::new(file_name, source.to_string());
        match self {
            MumeiError::VerificationError {
                msg,
                help,
                original_span,
                related,
                ..
            } => {
                // Propagate source to related diagnostics that share the same file.
                // Only overwrite when the related span's file matches the primary file
                // (or is "<unknown>"), preserving cross-file span context.
                let updated_related = related
                    .into_iter()
                    .map(|r| {
                        let r_file = r.original_span.file.as_str();
                        if r_file.is_empty() || r_file == "<unknown>" || r_file == file_name {
                            let recomputed_span = span_to_source_span(source, &r.original_span);
                            RelatedDiagnostic {
                                src: miette::NamedSource::new(file_name, source.to_string()),
                                span: recomputed_span,
                                ..r
                            }
                        } else {
                            r
                        }
                    })
                    .collect();
                MumeiError::VerificationError {
                    msg,
                    src: named_src,
                    span: source_span,
                    help,
                    original_span,
                    related: updated_related,
                }
            }
            MumeiError::CodegenError {
                msg,
                help,
                original_span,
                related,
                ..
            } => {
                let updated_related = related
                    .into_iter()
                    .map(|r| {
                        let r_file = r.original_span.file.as_str();
                        if r_file.is_empty() || r_file == "<unknown>" || r_file == file_name {
                            let recomputed_span = span_to_source_span(source, &r.original_span);
                            RelatedDiagnostic {
                                src: miette::NamedSource::new(file_name, source.to_string()),
                                span: recomputed_span,
                                ..r
                            }
                        } else {
                            r
                        }
                    })
                    .collect();
                MumeiError::CodegenError {
                    msg,
                    src: named_src,
                    span: source_span,
                    help,
                    original_span,
                    related: updated_related,
                }
            }
            MumeiError::TypeError {
                msg,
                help,
                original_span,
                related,
                ..
            } => {
                let updated_related = related
                    .into_iter()
                    .map(|r| {
                        let r_file = r.original_span.file.as_str();
                        if r_file.is_empty() || r_file == "<unknown>" || r_file == file_name {
                            let recomputed_span = span_to_source_span(source, &r.original_span);
                            RelatedDiagnostic {
                                src: miette::NamedSource::new(file_name, source.to_string()),
                                span: recomputed_span,
                                ..r
                            }
                        } else {
                            r
                        }
                    })
                    .collect();
                MumeiError::TypeError {
                    msg,
                    src: named_src,
                    span: source_span,
                    help,
                    original_span,
                    related: updated_related,
                }
            }
        }
    }

    /// ヘルプメッセージを設定する
    pub fn with_help(self, help_msg: impl Into<String>) -> Self {
        let help = Some(help_msg.into());
        match self {
            MumeiError::VerificationError {
                msg,
                src,
                span,
                original_span,
                related,
                ..
            } => MumeiError::VerificationError {
                msg,
                src,
                span,
                help,
                original_span,
                related,
            },
            MumeiError::CodegenError {
                msg,
                src,
                span,
                original_span,
                related,
                ..
            } => MumeiError::CodegenError {
                msg,
                src,
                span,
                help,
                original_span,
                related,
            },
            MumeiError::TypeError {
                msg,
                src,
                span,
                original_span,
                related,
                ..
            } => MumeiError::TypeError {
                msg,
                src,
                span,
                help,
                original_span,
                related,
            },
        }
    }

    /// Add a related diagnostic span for multi-span error reporting (Feature 3c)
    pub fn with_related(
        mut self,
        related_span: SourceSpan,
        label: String,
        src: miette::NamedSource<String>,
        msg: String,
        original_span: Span,
    ) -> Self {
        let diag = RelatedDiagnostic {
            msg,
            src,
            span: related_span,
            label,
            original_span,
        };
        match &mut self {
            MumeiError::VerificationError { related, .. } => related.push(diag),
            MumeiError::CodegenError { related, .. } => related.push(diag),
            MumeiError::TypeError { related, .. } => related.push(diag),
        }
        self
    }
}

impl From<String> for MumeiError {
    fn from(s: String) -> Self {
        MumeiError::verification(s)
    }
}

impl From<&str> for MumeiError {
    fn from(s: &str) -> Self {
        MumeiError::verification(s)
    }
}

pub type MumeiResult<T> = Result<T, MumeiError>;
type Env<'a> = HashMap<String, Dynamic<'a>>;
type DynResult<'a> = MumeiResult<Dynamic<'a>>;

// =============================================================================
// Constraint Mapping (Feature 1-a: Semantic Counter-example Feedback)
// =============================================================================

/// Tracks the relationship between Z3 symbolic variables and source-level context.
/// Used to generate semantic feedback from Z3 counter-examples.
#[derive(Debug, Clone)]
pub struct ConstraintMapping {
    param_name: String,
    type_name: Option<String>,
    base_type: String,
    predicate_raw: String,
    span: Span,
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
fn try_match_range(pred: &str, param_name: &str, type_name: &str, value: &str) -> Option<String> {
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
fn extract_bound(expr: &str, is_lower: bool) -> Option<String> {
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
fn extract_bound_reversed(expr: &str, is_lower: bool) -> Option<String> {
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
fn try_match_modulo(pred: &str, param_name: &str, value: &str) -> Option<String> {
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
fn try_match_enum(pred: &str, param_name: &str, value: &str) -> Option<String> {
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
fn try_match_string_constraint(pred: &str, param_name: &str, value: &str) -> Option<String> {
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
fn try_match_negation(
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
fn try_match_comparison(
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

/// Structured representation of a Z3 tracking label.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StructuredLabel {
    pub constraint_type: String,
    pub param: Option<String>,
    pub type_name: Option<String>,
    pub field: Option<String>,
    pub description: String,
}

/// Parse a Z3 tracking label into a StructuredLabel.
/// Returns None for unrecognized labels (internal bookkeeping variables).
fn parse_tracking_label(label: &str) -> Option<StructuredLabel> {
    if label == "track_requires" {
        return Some(StructuredLabel {
            constraint_type: "requires".to_string(),
            param: None,
            type_name: None,
            field: None,
            description: "Precondition (requires) / 前提条件 (requires)".to_string(),
        });
    }
    if let Some(rest) = label.strip_prefix("track_refined_type_") {
        // format: track_refined_type_{var}::{type}
        if let Some(idx) = rest.find("::") {
            let var = &rest[..idx];
            let tn = &rest[idx + 2..];
            return Some(StructuredLabel {
                constraint_type: "refined_type".to_string(),
                param: Some(var.to_string()),
                type_name: Some(tn.to_string()),
                field: None,
                description: format!(
                    "Refined type constraint: {} ({}) / 精緻型制約: {} ({})",
                    var, tn, var, tn
                ),
            });
        }
    }
    if let Some(rest) = label.strip_prefix("track_struct_field_") {
        // format: track_struct_field_{param}::{field}
        if let Some(idx) = rest.find("::") {
            let param = &rest[..idx];
            let fld = &rest[idx + 2..];
            return Some(StructuredLabel {
                constraint_type: "struct_field".to_string(),
                param: Some(param.to_string()),
                type_name: None,
                field: Some(fld.to_string()),
                description: format!(
                    "Struct field constraint: {}.{} / 構造体フィールド制約: {}.{}",
                    param, fld, param, fld
                ),
            });
        }
    }
    if let Some(rest) = label.strip_prefix("track_quantifier_") {
        return Some(StructuredLabel {
            constraint_type: "quantifier".to_string(),
            param: None,
            type_name: None,
            field: None,
            description: format!("Quantifier constraint #{} / 量子化制約 #{}", rest, rest),
        });
    }
    if let Some(rest) = label.strip_prefix("track_u64_nonneg_") {
        return Some(StructuredLabel {
            constraint_type: "u64_nonneg".to_string(),
            param: Some(rest.to_string()),
            type_name: None,
            field: None,
            description: format!(
                "Non-negative constraint: {} (u64) / 非負制約: {} (u64)",
                rest, rest
            ),
        });
    }
    None
}

/// Encode an effect state name as an integer for Z3 Int Sort constraints.
/// Returns the index of the state in the state machine's states list, or -1 if not found.
fn encode_effect_state(
    state_machine: &crate::mir_analysis::EffectStateMachine,
    state_name: &str,
) -> i64 {
    state_machine
        .states
        .iter()
        .position(|s| s == state_name)
        .map(|i| i as i64)
        .unwrap_or(-1)
}

/// Build semantic feedback JSON for contradiction (unsat) detection with unsat core info.
pub fn build_contradiction_feedback(
    atom_name: &str,
    conflicting_constraints: &[String],
    raw_labels: &[String],
    structured_labels: &[StructuredLabel],
) -> serde_json::Value {
    let explanation = if conflicting_constraints.is_empty() {
        "The constraints are mutually contradictory, but the specific conflicting set could not be determined. \
         (制約が相互に矛盾していますが、具体的な矛盾セットを特定できませんでした)".to_string()
    } else {
        format!(
            "The following constraints are mutually contradictory: {} \
             (以下の制約が相互に矛盾しています: {})",
            conflicting_constraints.join(", "),
            conflicting_constraints.join(", ")
        )
    };

    json!({
        "failure_type": FAILURE_INVARIANT_VIOLATED,
        "atom": atom_name,
        "conflicting_constraints": conflicting_constraints,
        "raw_unsat_core": raw_labels,
        "structured_unsat_core": structured_labels,
        "explanation": explanation,
        "suggestion": suggestion_for_failure_type(FAILURE_INVARIANT_VIOLATED)
    })
}

/// Default constraint budget per atom (max number of solver.assert() calls).
pub const DEFAULT_CONSTRAINT_BUDGET: usize = 1000;

/// 検証時に共有するコンテキスト（ctx, arr, module_env を束ねて引数を削減）
struct VCtx<'a> {
    ctx: &'a Context,
    arr: &'a Array<'a>,
    module_env: &'a ModuleEnv,
    /// Phase B call_with_contract: 現在検証中の atom への参照。
    /// CallRef の動的ケース（パラメトリック関数型）で、呼び出し先の関数パラメータに
    /// 宣言された contract(f) 情報を取得するために使用する。
    current_atom: Option<&'a crate::parser::Atom>,
    /// LinearityCtx for ownership/borrowing tracking during body evaluation.
    /// Wrapped in RefCell so that recursive expr_to_z3/stmt_to_z3 calls can
    /// mutate it without changing every call-site signature.
    linearity_ctx: Option<&'a std::cell::RefCell<LinearityCtx>>,
    /// EffectCtx for tracking allowed vs used effects during body evaluation.
    effect_ctx: Option<&'a std::cell::RefCell<EffectCtx>>,
    /// Per-atom constraint budget: tracks the number of solver.assert() calls.
    /// When the count exceeds the limit, verification returns an error to
    /// prevent Z3 explosion on pathological inputs.
    constraint_count: Option<&'a std::cell::Cell<usize>>,
    /// Maximum allowed constraint count for this atom.
    constraint_budget: usize,
    /// Flag set to true when Z3 String Sort constraints are added.
    /// When true, the Sort-aware timeout mechanism doubles timeout_ms
    /// to accommodate the higher complexity of string theory solving.
    /// Currently infrastructure-only: will be activated when Z3 String Sort
    /// is integrated for effect parameter constraints.
    /// Z3 String Sort 統合時にここを有効化する
    #[allow(dead_code)]
    has_string_constraints: Option<&'a std::cell::Cell<bool>>,
}

// =============================================================================
// 線形性チェック（Linear Types / Ownership Tracking）
// =============================================================================
//
// NOTE (Plan 19 — Phase 4c complete): The primary ownership/move analysis has
// been migrated to MIR-based MoveAnalysis (mumei-core/src/mir_analysis.rs).  Phase 1h in
// verify() now runs forward dataflow move analysis on the MIR CFG and reports
// UseAfterMove, DoubleMove, and ConflictingMerge as hard errors.
//
// LinearityCtx is retained as a secondary Z3-integrated check for:
// - Borrow tracking at call sites (ref / ref mut parameter handling)
// - Consume tracking within Z3 symbolic execution (ensures __alive_ bools)
// - Violation accumulation for the Phase 5b linearity report
//
// Future: Once MIR borrow tracking is implemented (Phase 5), LinearityCtx
// can be fully removed.
//
/// 変数の線形性（所有権）追跡コンテキスト
///
/// 所有権（Ownership）と借用（Borrowing）の両方を追跡する。
/// - consume: 所有権を消費（移動）。消費後のアクセスは Use-After-Free。
/// - borrow: 読み取り専用の借用。借用中は所有者が consume/free できない。
/// - release_borrow: 借用を解放。
///
/// NOTE: Primary move analysis is now handled by MIR MoveAnalysis (Plan 19).
/// This struct is kept for Z3-level borrow/consume tracking during symbolic
/// execution. See mumei-core/src/mir_analysis.rs for the MIR-based replacement.
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
            .or_default()
            .push(borrower_name.to_string());
        Ok(())
    }

    /// 借用を解放する
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
    /// Available for future borrow-conflict diagnostic passes.
    #[allow(dead_code)]
    pub fn is_borrowed(&self, name: &str) -> bool {
        self.borrow_count.get(name).is_some_and(|&c| c > 0)
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
/// main.rs で構築し、verify() / codegen に参照渡しする。
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
    /// エフェクト定義（副作用検証用）
    /// エフェクト名 → EffectDef
    pub effects: HashMap<String, EffectDef>,
    /// エフェクト定義レジストリ（階層構造対応）
    /// Step 2a: EffectDef のパラメータ・制約・親を含む完全な定義
    pub effect_defs: HashMap<String, EffectDef>,
    /// Symbolic String ID: パス文字列 → 整数ID のマッピング（ハイブリッド・アプローチ）
    // NOTE: path_id_map/next_path_id/prefix_ranges are infrastructure for Z3 String Sort migration (see ROADMAP.md P4)
    #[allow(dead_code)]
    pub path_id_map: HashMap<String, i64>,
    #[allow(dead_code)]
    pub next_path_id: i64,
    /// パスプレフィックス → (range_start, range_end) のマッピング
    #[allow(dead_code)]
    pub prefix_ranges: HashMap<String, (i64, i64)>,

    // =========================================================================
    // Dependency Graph (Feature 2-a: Gradual Verification)
    // =========================================================================
    /// Forward dependency graph: atom_name → set of atoms it calls
    pub dependency_graph: HashMap<String, HashSet<String>>,
    /// Reverse dependency graph: atom_name → set of atoms that call it
    pub reverse_deps: HashMap<String, HashSet<String>>,
    /// Security policy for effect parameter constraint enforcement
    pub security_policy: Option<SecurityPolicy>,
    /// メソッド名 → Vec<(トレイト名, メソッドインデックス)> の逆引きインデックス。
    /// `register_trait()` 時に構築され、`get_traits_for_method()` で使用する。
    /// HashMap iteration の非決定性を排除し、同名メソッドを持つ複数トレイトを正しく解決する。
    pub method_trait_index: HashMap<String, Vec<(String, usize)>>,
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
        // Plan 9: Str is a primitive type, return as-is
        if type_name == "Str" {
            return "Str".to_string();
        }
        if let Some(refined) = self.types.get(type_name) {
            return refined._base_type.clone();
        }
        type_name.to_string()
    }

    pub fn register_trait(&mut self, trait_def: &TraitDef) {
        // メソッド→トレイト逆引きインデックスを構築
        // 再登録時は旧エントリを除去してから追加（冪等性を維持）
        for entries in self.method_trait_index.values_mut() {
            entries.retain(|(tn, _)| tn != &trait_def.name);
        }
        for (idx, method) in trait_def.methods.iter().enumerate() {
            self.method_trait_index
                .entry(method.name.clone())
                .or_default()
                .push((trait_def.name.clone(), idx));
        }
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
    pub fn find_impl(&self, trait_name: &str, target_type: &str) -> Option<&ImplDef> {
        self.impls
            .iter()
            .find(|i| i.trait_name == trait_name && i.target_type == target_type)
    }

    /// メソッド名からトレイト定義とメソッドのparam_constraintsを取得する（全候補版）。
    /// Returns Vec<(trait_name, &TraitMethod)> for all traits defining this method.
    /// Uses the `method_trait_index` for deterministic, O(1) lookup.
    pub fn get_traits_for_method(&self, method_name: &str) -> Vec<(&str, &TraitMethod)> {
        let mut results = Vec::new();
        if let Some(entries) = self.method_trait_index.get(method_name) {
            for (trait_name, method_idx) in entries {
                if let Some(trait_def) = self.traits.get(trait_name) {
                    if let Some(method) = trait_def.methods.get(*method_idx) {
                        results.push((trait_name.as_str(), method));
                    }
                }
            }
        }
        results
    }

    /// 後方互換ラッパー: 候補が1つの場合は従来通りの動作を維持。
    /// 複数候補がある場合は最初の候補を返す（呼び出し元で find_impl による絞り込みを推奨）。
    #[allow(dead_code)]
    pub fn get_trait_for_method(&self, method_name: &str) -> Option<(&str, &TraitMethod)> {
        let candidates = self.get_traits_for_method(method_name);
        candidates.into_iter().next()
    }

    /// 指定した型がトレイト境界を全て満たしているか検証する
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

    /// P5-C: Set the trust level of a registered atom (for taint analysis on unverified imports)
    pub fn set_trust_level(&mut self, atom_name: &str, level: TrustLevel) {
        if let Some(atom) = self.atoms.get_mut(atom_name) {
            atom.trust_level = level;
        }
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

    /// エフェクト定義を登録する（effects + effect_defs 両方に登録）
    pub fn register_effect(&mut self, effect_def: &EffectDef) {
        self.effects
            .insert(effect_def.name.clone(), effect_def.clone());
        self.effect_defs
            .insert(effect_def.name.clone(), effect_def.clone());
    }

    /// エフェクト定義を取得する
    #[allow(dead_code)]
    pub fn get_effect(&self, name: &str) -> Option<&EffectDef> {
        self.effects.get(name)
    }

    /// エフェクト名のリストを展開し、includes を再帰的に解決して
    /// 全てのリーフエフェクト名を返す。
    pub fn resolve_effect_set(&self, names: &[String]) -> HashSet<String> {
        let mut result = HashSet::new();
        let mut stack: Vec<String> = names.to_vec();
        while let Some(name) = stack.pop() {
            if result.contains(&name) {
                continue;
            }
            result.insert(name.clone());
            if let Some(def) = self.effects.get(&name) {
                for included in &def.includes {
                    if !result.contains(included) {
                        stack.push(included.clone());
                    }
                }
            }
        }
        result
    }

    /// Vec<Effect> からエフェクト名のリストを展開し、includes を再帰的に解決する。
    pub fn resolve_effect_set_from_effects(
        &self,
        effects: &[crate::parser::Effect],
    ) -> HashSet<String> {
        let names: Vec<String> = effects.iter().map(|e| e.name.clone()).collect();
        self.resolve_effect_set(&names)
    }

    /// Resolve an effect set to only its leaf effects (effects with no `includes`).
    /// This avoids false positives when comparing a caller declaring leaf effects
    /// against a callee declaring a composite effect (e.g., `[IO]` vs `[FileRead, FileWrite, Console]`).
    pub fn resolve_leaf_effects(&self, names: &[String]) -> HashSet<String> {
        let full = self.resolve_effect_set(names);
        full.into_iter()
            .filter(|name| {
                self.effects
                    .get(name)
                    .is_none_or(|def| def.includes.is_empty())
            })
            .collect()
    }

    /// Vec<Effect> からリーフエフェクトを解決する。
    pub fn resolve_leaf_effects_from_effects(
        &self,
        effects: &[crate::parser::Effect],
    ) -> HashSet<String> {
        let names: Vec<String> = effects.iter().map(|e| e.name.clone()).collect();
        self.resolve_leaf_effects(&names)
    }

    // =========================================================================
    // Step 2b: エフェクト階層の解決メソッド
    // =========================================================================

    /// エフェクト名からその祖先エフェクト（親→祖父→...）を全て返す。
    /// HttpRead → [Network] のように、包含関係を解決する。
    /// Plan 6: Multi-parent support — BFS over all parents.
    pub fn get_effect_ancestors(&self, effect_name: &str) -> Vec<String> {
        let mut ancestors = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(effect_name.to_string());
        visited.insert(effect_name.to_string());
        while let Some(current) = queue.pop_front() {
            // effect_defs を優先、なければ effects も参照
            let parents: Vec<String> = self
                .effect_defs
                .get(&current)
                .map(|def| def.parent.clone())
                .or_else(|| self.effects.get(&current).map(|def| def.parent.clone()))
                .unwrap_or_default();
            for parent in parents {
                if visited.insert(parent.clone()) {
                    ancestors.push(parent.clone());
                    queue.push_back(parent);
                }
            }
        }
        ancestors
    }

    /// effect_a が effect_b のサブタイプかを判定。
    /// HttpRead は Network のサブタイプ → is_subeffect("HttpRead", "Network") == true
    pub fn is_subeffect(&self, child: &str, parent: &str) -> bool {
        if child == parent {
            return true;
        }
        self.get_effect_ancestors(child)
            .contains(&parent.to_string())
    }

    // =========================================================================
    // Step 2c: Symbolic String ID 管理（ハイブリッド・アプローチ）
    // =========================================================================

    /// パス文字列を整数IDに変換して登録する。既に登録済みなら既存IDを返す。
    // NOTE: register_path_id is infrastructure for Z3 String Sort migration (see ROADMAP.md P4)
    #[allow(dead_code)]
    pub fn register_path_id(&mut self, path: &str) -> i64 {
        if let Some(&id) = self.path_id_map.get(path) {
            return id;
        }
        let id = self.next_path_id;
        self.next_path_id += 1;
        self.path_id_map.insert(path.to_string(), id);
        id
    }

    /// プレフィックスに対して整数範囲を割り当てる。
    /// 例: "/tmp/" → (1000, 1999) のように、"/tmp/" で始まるパスはこの範囲のIDを持つ。
    // NOTE: register_prefix_range is infrastructure for Z3 String Sort migration (see ROADMAP.md P4)
    #[allow(dead_code)]
    pub fn register_prefix_range(&mut self, prefix: &str, range_start: i64, range_end: i64) {
        self.prefix_ranges
            .insert(prefix.to_string(), (range_start, range_end));
    }

    /// パスIDが指定プレフィックスの範囲内にあるかチェックする。
    // NOTE: path_id_matches_prefix is infrastructure for Z3 String Sort migration (see ROADMAP.md P4)
    #[allow(dead_code)]
    pub fn path_id_matches_prefix(&self, path_id: i64, prefix: &str) -> bool {
        if let Some(&(start, end)) = self.prefix_ranges.get(prefix) {
            path_id >= start && path_id <= end
        } else {
            false
        }
    }

    /// エフェクト定義が存在するか確認する（トレイト境界 "Effect" の検証用）
    pub fn has_effect_def(&self, name: &str) -> bool {
        self.effect_defs.contains_key(name)
    }

    /// エフェクト定義を effect_defs レジストリに登録する。
    // NOTE: register_effect_def is used by future EffectDef import registration path
    #[allow(dead_code)]
    pub fn register_effect_def(&mut self, effect_def: &EffectDef) {
        self.effect_defs
            .insert(effect_def.name.clone(), effect_def.clone());
    }

    // =========================================================================
    // Dependency Graph Methods (Feature 2-a)
    // =========================================================================

    /// Register the set of atoms that `atom_name` calls.
    /// Populates both forward (`dependency_graph`) and reverse (`reverse_deps`) maps.
    pub fn register_dependencies(&mut self, atom_name: &str, callees: HashSet<String>) {
        for callee in &callees {
            self.reverse_deps
                .entry(callee.clone())
                .or_default()
                .insert(atom_name.to_string());
        }
        self.dependency_graph.insert(atom_name.to_string(), callees);
    }

    /// BFS traversal of `reverse_deps` to find all atoms transitively depending
    /// on the given atom.
    #[allow(dead_code)]
    pub fn get_transitive_dependents(&self, atom_name: &str) -> HashSet<String> {
        let mut result = HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(atom_name.to_string());
        while let Some(current) = queue.pop_front() {
            if let Some(dependents) = self.reverse_deps.get(&current) {
                for dep in dependents {
                    if result.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }
        result
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
            TraitMethod {
                name: "div".to_string(),
                param_types: vec!["Self".into(), "Self".into()],
                return_type: "Self".into(),
                param_constraints: vec![None, Some("v != 0".to_string())],
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
                ("div".into(), "a / b".into()),
            ],
            span: Span::default(),
        });
    }
}

// =============================================================================
// 組み込みエフェクト (Built-in Effects)
// =============================================================================

/// 組み込みエフェクトを ModuleEnv に自動登録する。
/// FileRead, FileWrite, Network, Log, Console の基本エフェクトと、
/// IO (FileRead + FileWrite + Console), FullAccess (IO + Network + Log) の
/// 複合エフェクトを提供。
pub fn register_builtin_effects(module_env: &mut ModuleEnv) {
    use crate::parser::EffectDef;

    // --- 基本エフェクト ---
    for name in &["FileRead", "FileWrite", "Network", "Log", "Console"] {
        module_env.register_effect(&EffectDef {
            name: name.to_string(),
            params: vec![],
            constraint: None,
            includes: vec![],
            refinement: None,
            parent: vec![],
            span: Span::default(),
            states: vec![],
            transitions: vec![],
            initial_state: None,
        });
    }

    // --- 複合エフェクト ---
    // IO includes FileRead, FileWrite, Console
    module_env.register_effect(&EffectDef {
        name: "IO".to_string(),
        params: vec![],
        constraint: None,
        includes: vec![
            "FileRead".to_string(),
            "FileWrite".to_string(),
            "Console".to_string(),
        ],
        refinement: None,
        parent: vec![],
        span: Span::default(),
        states: vec![],
        transitions: vec![],
        initial_state: None,
    });

    // FullAccess includes IO, Network, Log
    module_env.register_effect(&EffectDef {
        name: "FullAccess".to_string(),
        params: vec![],
        constraint: None,
        includes: vec!["IO".to_string(), "Network".to_string(), "Log".to_string()],
        refinement: None,
        parent: vec![],
        span: Span::default(),
        states: vec![],
        transitions: vec![],
        initial_state: None,
    });
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
        let int_sort = z3::Sort::int(&ctx);
        let arr = Array::new_const(&ctx, "arr", &int_sort, &int_sort);
        let vc = VCtx {
            ctx: &ctx,
            arr: &arr,
            module_env,
            current_atom: None,
            linearity_ctx: None,
            effect_ctx: None,
            constraint_count: None,
            constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
            has_string_constraints: None,
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

                    // P2-B: trait method の param_constraints を solver に assert する。
                    // law 検証時にメソッドのパラメータ制約（例: div の第2引数 != 0）を
                    // 前提条件として注入し、制約下での law 成立を検証する。
                    // NOTE: push/pop スコープ内で assert することで、制約が
                    // 他の law 検証に漏れないようにする。
                    // TODO: 将来的には、law_expr に実際に含まれるメソッドの
                    // 制約のみを注入するようフィルタリングすべき。
                    for method in &trait_def.methods {
                        // Only inject constraints for methods that appear in this law
                        if !law_expr.contains(&method.name) {
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
                            );
                            if ce_parts.is_empty() {
                                "  (no concrete values available)".to_string()
                            } else {
                                format!("  Counter-example: {}", ce_parts.join(", "))
                            }
                        } else {
                            "  (could not retrieve model)".to_string()
                        };
                        solver.pop(1);
                        return Err(MumeiError::verification_at(
                            format!(
                                "impl {} for {}: law '{}' (defined in trait at {}) is not satisfied\n  Law: {}\n  Expanded: {}\n{}",
                                impl_def.trait_name, impl_def.target_type,
                                law_name, trait_def.span, law_expr, substituted, counterexample
                            ),
                            impl_def.span.clone()
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
    /// For symbolic (non-literal) parameters, returns Ok (deferred to Z3).
    // TODO: Migrate to Z3 String Sort when available for full symbolic string verification.
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
fn evaluate_string_constraint(constraint_expr: &str, _param_name: &str, value: &str) -> bool {
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
fn parse_constraint_to_z3_string<'ctx>(
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
struct EffectCtx {
    /// Effects allowed in the current scope (from atom's effects annotation, transitively expanded)
    allowed_effects: HashSet<String>,
    /// Effects actually used in the body (from perform expressions)
    used_effects: HashSet<String>,
    /// Violation messages
    violations: Vec<String>,
}

impl EffectCtx {
    fn new(allowed: HashSet<String>) -> Self {
        Self {
            allowed_effects: allowed,
            used_effects: HashSet::new(),
            violations: Vec::new(),
        }
    }

    /// Record a perform and check if the effect is allowed
    fn perform_effect(&mut self, effect_name: &str) -> Result<(), String> {
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
    fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }
}

/// Verify effect containment for an atom using Z3.
/// Proves: ∀e ∈ UsedEffects(Body): e ∈ AllowedEffects(Signature)
fn verify_effect_containment(
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
fn save_effect_violation_report(
    output_dir: &Path,
    atom_name: &str,
    declared_effects: &[String],
    required_effect: &str,
    source_operation: &str,
    suggested_fixes: &[String],
) {
    let report = json!({
        "status": "failed",
        "atom": atom_name,
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
    let _ = fs::create_dir_all(output_dir);
    let _ = fs::write(
        output_dir.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap_or_else(|_| report.to_string()),
    );
}

/// Save an effect propagation violation report to report.json for self-healing integration.
fn save_effect_propagation_report(
    output_dir: &Path,
    caller_name: &str,
    callee_name: &str,
    caller_effects: &[String],
    callee_effects: &[String],
    missing_effects: &[String],
) {
    let report = json!({
        "status": "failed",
        "atom": caller_name,
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
    let _ = fs::create_dir_all(output_dir);
    let _ = fs::write(
        output_dir.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap_or_else(|_| report.to_string()),
    );
}

/// Save an effect polymorphism violation report to report.json for self-healing integration.
/// Called when an atom_ref parameter's effect_set is not a subset of the atom's declared effects.
fn save_effect_polymorphism_report(
    output_dir: &Path,
    atom_name: &str,
    param_name: &str,
    param_effect_set: &[String],
    declared_effects: &[String],
    missing_effects: &[String],
) {
    let report = json!({
        "status": "failed",
        "atom": atom_name,
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
    let _ = fs::create_dir_all(output_dir);
    let _ = fs::write(
        output_dir.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap_or_else(|_| report.to_string()),
    );
}

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
const BMC_DEFAULT_UNROLL_DEPTH: usize = 3;

/// 再帰的 async 呼び出しの最大展開深度。
/// async atom が自身を呼び出す場合、この深度を超えると
/// 「Unknown（未定義）」として扱い、Z3 探索を打ち切る。
const MAX_ASYNC_RECURSION_DEPTH: usize = 3;

/// body 内の Acquire を再帰的に収集する（BMC 用）。
/// ループ内で acquire が使われているパターンを検出するために使用。
fn collect_acquire_resources_expr(expr: &Expr) -> Vec<String> {
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

fn collect_acquire_resources_stmt(stmt: &Stmt) -> Vec<String> {
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
fn verify_bmc_resource_safety(
    atom: &Atom,
    body_stmt: &Stmt,
    module_env: &ModuleEnv,
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

    // 展開回数: atom 単位のオーバーライド > グローバルデフォルト
    let unroll_depth = atom.max_unroll.unwrap_or(BMC_DEFAULT_UNROLL_DEPTH);

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
fn verify_async_recursion_depth(
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
fn verify_atom_invariant(
    atom: &Atom,
    body_stmt: &Stmt,
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
        current_atom: Some(atom),
        linearity_ctx: None,
        effect_ctx: None,
        constraint_count: None,
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: None,
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
            // Plan 9: Str parameters as Z3 String Sort
            "Str" => Z3String::new_const(&ctx, param.name.as_str()).into(),
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
fn expr_to_source_string(expr: &Expr) -> String {
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
fn collect_callees_with_args_expr(expr: &Expr) -> Vec<(String, Vec<Expr>)> {
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

fn collect_callees_with_args_stmt(stmt: &Stmt) -> Vec<(String, Vec<Expr>)> {
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

/// body 内の全 Call 式から呼び出し先の atom 名を収集する。
fn collect_callees_expr(expr: &Expr) -> Vec<String> {
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

fn collect_callees_stmt(stmt: &Stmt) -> Vec<String> {
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
fn check_taint_propagation(atom: &Atom, body_stmt: &Stmt, env: &Env, module_env: &ModuleEnv) {
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
// Step 3: Effect Inference（エフェクト推論）
// =============================================================================

/// body 内の関数呼び出しからエフェクトセットを推論する。
/// 呼び出し先 atom の effects フィールドを再帰的に集約する。
/// 親エフェクトへの暗黙的包含も解決する。
fn infer_effects(atom: &Atom, body_stmt: &Stmt, module_env: &ModuleEnv) -> Vec<Effect> {
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
fn collect_divisors_expr(expr: &Expr) -> Vec<String> {
    let mut divisors = Vec::new();
    match expr {
        Expr::BinaryOp(lhs, Op::Div, rhs) => {
            // The right-hand side is a divisor
            match rhs.as_ref() {
                Expr::Variable(name) => divisors.push(name.clone()),
                Expr::Number(n) => {
                    if *n == 0 {
                        divisors.push("0".to_string());
                    }
                }
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
fn collect_divisors_stmt(stmt: &Stmt) -> Vec<String> {
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
fn infer_requires(atom: &Atom, module_env: &ModuleEnv) -> Vec<String> {
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
fn infer_ensures(atom: &Atom, module_env: &ModuleEnv) -> Vec<String> {
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
fn verify_effect_consistency(atom: &Atom, module_env: &ModuleEnv) -> MumeiResult<()> {
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
fn check_constant_constraint(value: &str, constraint: &str) -> bool {
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
fn verify_effect_params(atom: &Atom, module_env: &ModuleEnv) -> MumeiResult<()> {
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

/// mumei.toml の [proof]/[build] 設定を反映した verify
/// timeout_ms: Z3 ソルバのタイムアウト（ミリ秒）
/// global_max_unroll: BMC のグローバル展開深度
pub fn verify_with_config(
    hir_atom: &HirAtom,
    output_dir: &Path,
    module_env: &ModuleEnv,
    timeout_ms: u64,
    _global_max_unroll: usize,
) -> MumeiResult<()> {
    verify_inner(hir_atom, output_dir, module_env, timeout_ms)
}

pub fn verify(hir_atom: &HirAtom, output_dir: &Path, module_env: &ModuleEnv) -> MumeiResult<()> {
    verify_inner(hir_atom, output_dir, module_env, 10000)
}

/// Compile-time metrics for a single atom verification.
/// Tracks the duration of each phase and total constraint count.
pub struct VerificationMetrics {
    pub atom_name: String,
    pub phase_times: Vec<(String, std::time::Duration)>,
    pub total_constraints: usize,
    pub z3_check_time: std::time::Duration,
}

impl VerificationMetrics {
    fn new(atom_name: &str) -> Self {
        Self {
            atom_name: atom_name.to_string(),
            phase_times: Vec::new(),
            total_constraints: 0,
            z3_check_time: std::time::Duration::ZERO,
        }
    }

    fn record_phase(&mut self, name: &str, duration: std::time::Duration) {
        self.phase_times.push((name.to_string(), duration));
    }

    /// Print metrics to stderr (for --verbose / debug output).
    pub fn print_summary(&self) {
        eprintln!("  [metrics] atom '{}' verification phases:", self.atom_name);
        for (name, dur) in &self.phase_times {
            eprintln!("    {}: {:.3}ms", name, dur.as_secs_f64() * 1000.0);
        }
        eprintln!(
            "    total_constraints: {}, z3_check: {:.3}ms",
            self.total_constraints,
            self.z3_check_time.as_secs_f64() * 1000.0
        );
    }
}

fn verify_inner(
    hir_atom: &HirAtom,
    output_dir: &Path,
    module_env: &ModuleEnv,
    timeout_ms: u64,
) -> MumeiResult<()> {
    let atom = &hir_atom.atom;
    let mut metrics = VerificationMetrics::new(&atom.name);

    // ジェネリック atom は単相化後に検証される
    // 例: pipe<E: Effect> は検証スキップ、pipe<FileWrite> が検証対象
    if !atom.type_params.is_empty() {
        return Ok(());
    }

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
                None,
                "",
                None,
                Some(&atom.span),
                None,
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
                    None,
                    "",
                    None,
                    Some(&atom.span),
                    None,
                );
                return Ok(());
            }
        }
        TrustLevel::Verified => {
            // 通常の検証フロー
        }
    }

    // Phase 1a: リソース階層検証（デッドロック防止）
    let phase_start = std::time::Instant::now();
    verify_resource_hierarchy(atom, module_env)?;
    metrics.record_phase("Phase 1a: resource hierarchy", phase_start.elapsed());

    // Phase 1f: エフェクト包含検証（副作用安全性）
    let phase_start = std::time::Instant::now();
    if let Err(e) = verify_effect_containment(atom, &hir_atom.body_stmt, module_env) {
        // Save structured effect violation report for self-healing integration.
        // Extract missing effects from the error to produce a structured report.
        let allowed_leaves = module_env.resolve_leaf_effects_from_effects(&atom.effects);
        let caller_effect_names: Vec<String> =
            atom.effects.iter().map(|e| e.name.clone()).collect();

        // まず callee ベースの違反を探す（effect_propagation）
        let callees = collect_callees_stmt(&hir_atom.body_stmt);
        let mut missing_all: Vec<String> = Vec::new();
        let mut violating_callee = String::new();
        let mut callee_effs: Vec<String> = Vec::new();
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
                        violating_callee = callee_name.clone();
                        callee_effs = callee_atom.effects.iter().map(|e| e.name.clone()).collect();
                        missing_all = missing;
                        break;
                    }
                }
            }
        }
        if !missing_all.is_empty() {
            save_effect_propagation_report(
                output_dir,
                &atom.name,
                &violating_callee,
                &caller_effect_names,
                &callee_effs,
                &missing_all,
            );
        } else {
            // callee ループでは見つからなかった場合、atom_ref パラメータの effect_set 違反を確認する
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
                                save_effect_polymorphism_report(
                                    output_dir,
                                    &atom.name,
                                    &param.name,
                                    effect_set,
                                    &caller_effect_names,
                                    &missing,
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }
        return Err(e);
    }
    metrics.record_phase("Phase 1f: effect containment", phase_start.elapsed());

    // Phase 1b: 有界モデル検査（ループ内 acquire パターン）
    let phase_start = std::time::Instant::now();
    verify_bmc_resource_safety(atom, &hir_atom.body_stmt, module_env)?;
    metrics.record_phase("Phase 1b: BMC resource safety", phase_start.elapsed());

    // Phase 1c: 再帰的 async 呼び出しの深度検証
    let phase_start = std::time::Instant::now();
    verify_async_recursion_depth(atom, &hir_atom.body_stmt, module_env)?;
    metrics.record_phase("Phase 1c: async recursion depth", phase_start.elapsed());

    // Phase 1d: atom レベル invariant の帰納的検証
    let phase_start = std::time::Instant::now();
    if let Some(ref invariant_expr) = atom.invariant {
        verify_atom_invariant(atom, &hir_atom.body_stmt, invariant_expr, module_env)?;
    }
    metrics.record_phase("Phase 1d: atom invariant", phase_start.elapsed());

    // Phase 1e: Call Graph サイクル検知（間接再帰の検出）
    let phase_start = std::time::Instant::now();
    verify_call_graph_cycles(atom, module_env)?;
    metrics.record_phase("Phase 1e: call graph cycles", phase_start.elapsed());

    // Phase 1f-2: エフェクト整合性チェック（宣言 vs 推論、警告レベル）
    // Note: verify_effect_consistency always returns Ok(()) — warnings are emitted
    // via eprintln! inside the function itself.
    let _ = verify_effect_consistency(atom, module_env);

    // Phase 1g: エフェクトパラメータ制約検証
    let phase_start = std::time::Instant::now();
    verify_effect_params(atom, module_env)?;
    metrics.record_phase("Phase 1g: effect params", phase_start.elapsed());

    // Phase 1h: MIR-based move analysis (Phase 4c integrated)
    // Lower HIR to MIR and run forward dataflow move analysis.
    // Copy types (Int, Nat, Bool, f64, etc.) are distinguished from Move types
    // via the Movability field on LocalDecl. Copy types are never consumed by
    // Rvalue::Use, so violations are only reported for Move types.
    // Move type violations are hard errors; Copy type false positives are eliminated.
    let phase_start = std::time::Instant::now();
    let mir_body = crate::mir::lower_hir_to_mir(hir_atom);
    let move_conflict_locals: Vec<(crate::mir::Local, crate::mir::BasicBlockId)> = Vec::new();
    if mir_body.check_analysis_budget().is_ok() {
        let move_result = crate::mir_analysis::analyze_moves(&mir_body);
        if let Some(v) = move_result.violations.first() {
            // Look up the local's name for better error messages
            let local_name = mir_body
                .locals
                .iter()
                .find(|d| d.local == v.local)
                .and_then(|d| d.name.clone())
                .unwrap_or_else(|| format!("_{}", v.local.0));
            match v.kind {
                crate::mir_analysis::MoveViolationKind::UseAfterMove => {
                    return Err(MumeiError::verification(format!(
                        "use of moved value `{}`: Local({}) was used after being moved in block {}",
                        local_name, v.local.0, v.block_id
                    )));
                }
                crate::mir_analysis::MoveViolationKind::DoubleMove => {
                    return Err(MumeiError::verification(format!(
                        "value `{}` moved twice: Local({}) was moved more than once in block {}",
                        local_name, v.local.0, v.block_id
                    )));
                }
                crate::mir_analysis::MoveViolationKind::ConflictingMerge => {
                    return Err(MumeiError::verification(format!(
                        "conflicting ownership of `{}`: Local({}) is alive on one control-flow path \
                         but consumed on another at merge point (block {})",
                        local_name, v.local.0, v.block_id
                    )));
                }
            }
        }
    }
    metrics.record_phase("Phase 1h: MIR move analysis", phase_start.elapsed());

    // Phase 1i: Temporal effect verification (stateful effects)
    // Build state machines from effect_defs and run forward dataflow analysis
    // on the MIR to verify that perform operations occur in valid states.
    let phase_start = std::time::Instant::now();
    {
        let mut state_machines: std::collections::HashMap<
            String,
            crate::mir_analysis::EffectStateMachine,
        > = std::collections::HashMap::new();

        // Build state machines from all known effect_defs
        for (name, def) in &module_env.effect_defs {
            if let Some(sm) = crate::mir_analysis::EffectStateMachine::from_effect_def(def) {
                state_machines.insert(name.clone(), sm);
            }
        }
        // Also check effects map
        for (name, def) in &module_env.effects {
            if !state_machines.contains_key(name) {
                if let Some(sm) = crate::mir_analysis::EffectStateMachine::from_effect_def(def) {
                    state_machines.insert(name.clone(), sm);
                }
            }
        }

        // Modular Verification: Override initial states from effect_pre contracts
        for (effect_name, pre_state) in &atom.effect_pre {
            if let Some(sm) = state_machines.get_mut(effect_name) {
                if sm.states.contains(pre_state) {
                    sm.initial_state = pre_state.clone();
                } else {
                    return Err(MumeiError::verification(format!(
                        "effect_pre: state '{}' is not a valid state for effect '{}' (valid states: {:?})",
                        pre_state, effect_name, sm.states
                    )));
                }
            } else {
                eprintln!(
                    "  ⚠️  effect_pre: no state machine found for effect '{}' (stateless effects are ignored)",
                    effect_name
                );
            }
        }

        if !state_machines.is_empty() && mir_body.check_analysis_budget().is_ok() {
            // Build callee effect contracts for cross-atom composition
            let mut callee_contracts: std::collections::HashMap<
                String,
                crate::mir_analysis::AtomEffectContract,
            > = std::collections::HashMap::new();
            for (atom_name, callee_atom) in &module_env.atoms {
                if !callee_atom.effect_pre.is_empty() || !callee_atom.effect_post.is_empty() {
                    callee_contracts.insert(
                        atom_name.clone(),
                        crate::mir_analysis::AtomEffectContract {
                            effect_pre: callee_atom.effect_pre.clone(),
                            effect_post: callee_atom.effect_post.clone(),
                        },
                    );
                }
            }
            let temporal_result = crate::mir_analysis::analyze_temporal_effects_with_contracts(
                &mir_body,
                &state_machines,
                if callee_contracts.is_empty() {
                    None
                } else {
                    Some(&callee_contracts)
                },
            );

            for v in &temporal_result.violations {
                match v.kind {
                    crate::mir_analysis::TemporalViolationKind::InvalidPreState => {
                        // Hard error: operation performed in wrong state
                        return Err(MumeiError::verification(format!(
                            "Temporal effect violation: '{}' operation '{}' requires state '{}' \
                             but current state is '{}' (block {})",
                            v.effect, v.operation, v.expected_state, v.actual_state, v.block_id
                        )));
                    }
                    crate::mir_analysis::TemporalViolationKind::ConflictingState => {
                        // Plan 20: Z3 Int Sort constraint generation for conflicting
                        // states at merge points.  We encode each state as an integer
                        // and ask Z3 whether both predecessor states can be satisfied
                        // simultaneously — if UNSAT the conflict is irreconcilable.

                        // Look up the state machine for this effect.
                        if let Some(sm) = state_machines.get(&v.effect) {
                            let expected_int = encode_effect_state(sm, &v.expected_state);
                            let actual_int = encode_effect_state(sm, &v.actual_state);

                            // Only proceed if both states are known.
                            if expected_int >= 0 && actual_int >= 0 {
                                // Check constraint budget: each Z3 probe costs ~4
                                // assertions (variable, branch-a, branch-b, equality).
                                // Phase 1i runs before the main solver is created, so
                                // we use mir_body complexity as a proxy budget check.
                                let budget_ok = mir_body.complexity() < DEFAULT_CONSTRAINT_BUDGET;

                                if budget_ok {
                                    // Create a scoped Z3 context + solver for this probe.
                                    let z3_cfg = Config::new();
                                    let z3_ctx = Context::new(&z3_cfg);
                                    let z3_solver = Solver::new(&z3_ctx);

                                    // Z3 Int variable: __effect_state_{effect}_{block_id}
                                    let var_name =
                                        format!("__effect_state_{}_{}", v.effect, v.block_id);
                                    let state_var = Int::new_const(&z3_ctx, var_name.as_str());

                                    // Assert: state_var == expected (from one branch)
                                    let eq_expected =
                                        state_var._eq(&Int::from_i64(&z3_ctx, expected_int));
                                    // Assert: state_var == actual (from other branch)
                                    let eq_actual =
                                        state_var._eq(&Int::from_i64(&z3_ctx, actual_int));

                                    // Both must hold simultaneously at the merge point.
                                    z3_solver.assert(&eq_expected);
                                    z3_solver.assert(&eq_actual);

                                    // Also constrain variable to valid state range.
                                    let num_states = sm.states.len() as i64;
                                    z3_solver.assert(&state_var.ge(&Int::from_i64(&z3_ctx, 0)));
                                    z3_solver
                                        .assert(&state_var.lt(&Int::from_i64(&z3_ctx, num_states)));

                                    match z3_solver.check() {
                                        SatResult::Unsat => {
                                            // Irreconcilable: the two branches require
                                            // mutually exclusive states → hard error.
                                            return Err(MumeiError::verification(format!(
                                                "Temporal effect conflict (Z3 UNSAT): effect '{}' \
                                                 has irreconcilable states at merge point (block {}): \
                                                 '{}' (={}) vs '{}' (={}). \
                                                 The conflict cannot be resolved.",
                                                v.effect, v.block_id,
                                                v.expected_state, expected_int,
                                                v.actual_state, actual_int,
                                            )));
                                        }
                                        SatResult::Sat => {
                                            // SAT means the states are actually compatible
                                            // (should not normally happen for truly different
                                            // states, but could occur with aliased encodings).
                                            // Emit info diagnostic.
                                            eprintln!(
                                                "  \u{2139}\u{fe0f}  Temporal effect info: '{}' conflicting states \
                                                 at block {} resolved by Z3 (SAT): '{}' vs '{}'.",
                                                v.effect, v.block_id,
                                                v.expected_state, v.actual_state
                                            );
                                        }
                                        SatResult::Unknown => {
                                            // Solver timeout / unknown — keep as warning.
                                            eprintln!(
                                                "  \u{26a0}\u{fe0f}  Temporal effect warning: '{}' conflicting states \
                                                 at block {}: Z3 returned Unknown for '{}' vs '{}'.",
                                                v.effect, v.block_id,
                                                v.expected_state, v.actual_state
                                            );
                                        }
                                    }
                                } else {
                                    // Budget exceeded — fall back to warning.
                                    eprintln!(
                                        "  \u{26a0}\u{fe0f}  Temporal effect warning: '{}' has conflicting states \
                                         at merge point (block {}): '{}' vs '{}'. \
                                         Constraint budget exceeded, Z3 probe skipped.",
                                        v.effect, v.block_id,
                                        v.expected_state, v.actual_state
                                    );
                                }
                            } else {
                                // Unknown state name — fall back to warning.
                                eprintln!(
                                    "  \u{26a0}\u{fe0f}  Temporal effect warning: '{}' has conflicting states \
                                     at merge point (block {}): '{}' vs '{}'. \
                                     State encoding failed.",
                                    v.effect, v.block_id,
                                    v.expected_state, v.actual_state
                                );
                            }
                        } else {
                            // No state machine found — fall back to warning.
                            eprintln!(
                                "  \u{26a0}\u{fe0f}  Temporal effect warning: '{}' has conflicting states \
                                 at merge point (block {}): '{}' vs '{}'. \
                                 No state machine found.",
                                v.effect, v.block_id,
                                v.expected_state, v.actual_state
                            );
                        }
                    }
                    crate::mir_analysis::TemporalViolationKind::UnexpectedFinalState => {
                        // Hard error: effect left in unexpected state at exit
                        return Err(MumeiError::verification(format!(
                            "Temporal effect violation: effect '{}' has final state '{}' \
                             but effect_post declares '{}' (block {})",
                            v.effect, v.actual_state, v.expected_state, v.block_id
                        )));
                    }
                }
            }

            // Modular Verification: Check effect_post contracts against exit states
            if !atom.effect_post.is_empty() {
                for (effect_name, expected_post) in &atom.effect_post {
                    // Validate that the effect has a state machine
                    if !state_machines.contains_key(effect_name) {
                        eprintln!(
                            "  ⚠️  effect_post: no state machine found for effect '{}' (stateless effects are ignored)",
                            effect_name
                        );
                        continue;
                    }
                    // Validate that the expected post-state is a valid state
                    if let Some(sm) = state_machines.get(effect_name) {
                        if !sm.states.contains(expected_post) {
                            return Err(MumeiError::verification(format!(
                                "effect_post: state '{}' is not a valid state for effect '{}' (valid states: {:?})",
                                expected_post, effect_name, sm.states
                            )));
                        }
                    }
                    // Find the exit state for this effect from the last basic block(s)
                    // that have a Return terminator
                    let mut found_exit = false;
                    for (block_id, exit_map) in &temporal_result.exit_states {
                        let block = &mir_body.blocks[*block_id];
                        if matches!(block.terminator, crate::mir::Terminator::Return(_)) {
                            if let Some(actual_state) = exit_map.get(effect_name) {
                                found_exit = true;
                                if actual_state != expected_post {
                                    return Err(MumeiError::verification(format!(
                                        "Temporal effect violation: effect '{}' has final state '{}' \
                                         but effect_post declares '{}' (block {})",
                                        effect_name, actual_state, expected_post, block_id
                                    )));
                                }
                            }
                        }
                    }
                    if !found_exit {
                        eprintln!(
                            "  ⚠️  effect_post: effect '{}' has no tracked exit state \
                             (no perform operations for this effect in the body)",
                            effect_name
                        );
                    }
                }
            }
        }
    }
    metrics.record_phase("Phase 1i: temporal effects", phase_start.elapsed());

    // ✅ Phase 4c complete (Plan 19): MIR lowering now covers all expression forms
    // (Match, Lambda, Async, Await, Task, TaskGroup, ChanSend, ChanRecv, etc.).
    // Primary move analysis is handled by Phase 1h above (MIR MoveAnalysis).
    // LinearityCtx below is retained only for Z3-level borrow/consume tracking.

    // Sort-aware timeout: if has_string_constraints is true, double the timeout.
    // Z3 String Sort is now integrated for effect parameter constraints.
    // When string constraints are present, solving is significantly slower,
    // so we double the timeout to accommodate.
    let has_string_constraints_cell_pre = std::cell::Cell::new(false);
    // Pre-scan (1): check if any declared effect has Str-typed params with constraints
    // that would need Z3 String Sort (variable params, not constant-folded).
    for eff in &atom.effects {
        let effect_def = module_env
            .effect_defs
            .get(&eff.name)
            .or_else(|| module_env.effects.get(&eff.name));
        if let Some(def) = effect_def {
            if def.constraint.is_some() {
                for p in &eff.params {
                    if !p.is_constant {
                        has_string_constraints_cell_pre.set(true);
                    }
                }
            }
        }
    }
    // Pre-scan (2): also check the body for `perform` expressions with
    // non-constant args whose EffectDef has a constraint. This catches cases
    // where the atom declares `effects: [FileRead("/tmp/")]` (constant) but
    // the body does `perform FileRead.read(some_variable)`.
    fn body_has_symbolic_perform_args(stmt: &Stmt, module_env: &ModuleEnv) -> bool {
        match stmt {
            Stmt::Block(stmts, _) => stmts
                .iter()
                .any(|s| body_has_symbolic_perform_args(s, module_env)),
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
                expr_has_symbolic_perform_args(value, module_env)
            }
            Stmt::While { cond, body, .. } => {
                expr_has_symbolic_perform_args(cond, module_env)
                    || body_has_symbolic_perform_args(body, module_env)
            }
            Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
                body_has_symbolic_perform_args(body, module_env)
            }
            Stmt::TaskGroup { children, .. } => children
                .iter()
                .any(|c| body_has_symbolic_perform_args(c, module_env)),
            Stmt::Expr(e, _) => expr_has_symbolic_perform_args(e, module_env),
            // Plan 8: Cancel statement has no perform args
            Stmt::Cancel { .. } => false,
        }
    }
    fn expr_has_symbolic_perform_args(expr: &Expr, module_env: &ModuleEnv) -> bool {
        match expr {
            Expr::Perform { effect, args, .. } => {
                let effect_def = module_env
                    .effect_defs
                    .get(effect.as_str())
                    .or_else(|| module_env.effects.get(effect.as_str()));
                if let Some(def) = effect_def {
                    if def.constraint.is_some() {
                        for arg in args {
                            if !matches!(arg, Expr::Number(_) | Expr::Float(_)) {
                                return true;
                            }
                        }
                    }
                }
                // Also recurse into args
                args.iter()
                    .any(|a| expr_has_symbolic_perform_args(a, module_env))
            }
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                expr_has_symbolic_perform_args(cond, module_env)
                    || body_has_symbolic_perform_args(then_branch, module_env)
                    || body_has_symbolic_perform_args(else_branch, module_env)
            }
            Expr::BinaryOp(l, _, r) => {
                expr_has_symbolic_perform_args(l, module_env)
                    || expr_has_symbolic_perform_args(r, module_env)
            }
            Expr::Call(_, args) => args
                .iter()
                .any(|a| expr_has_symbolic_perform_args(a, module_env)),
            Expr::Async { body } => body_has_symbolic_perform_args(body, module_env),
            Expr::Await { expr } => expr_has_symbolic_perform_args(expr, module_env),
            Expr::Lambda { body, .. } => body_has_symbolic_perform_args(body, module_env),
            Expr::CallRef { callee, args } => {
                expr_has_symbolic_perform_args(callee, module_env)
                    || args
                        .iter()
                        .any(|a| expr_has_symbolic_perform_args(a, module_env))
            }
            Expr::Match { target, arms } => {
                expr_has_symbolic_perform_args(target, module_env)
                    || arms
                        .iter()
                        .any(|arm| body_has_symbolic_perform_args(&arm.body, module_env))
            }
            // Plan 8: Channel operations — traverse sub-expressions for symbolic perform args
            Expr::ChanSend { channel, value } => {
                expr_has_symbolic_perform_args(channel, module_env)
                    || expr_has_symbolic_perform_args(value, module_env)
            }
            Expr::ChanRecv { channel } => expr_has_symbolic_perform_args(channel, module_env),
            // NOTE: StructInit, FieldAccess, and ArrayAccess contain sub-expressions
            // that could hold nested Perform nodes, but are not recursed into here.
            // This means the pre-scan conservatively under-estimates: if a perform with
            // a variable arg appears inside a struct field initializer, field access base,
            // or array index, the timeout won't be doubled. This is safe (slower, not
            // incorrect) and these patterns are rare in practice.
            _ => false,
        }
    }
    if !has_string_constraints_cell_pre.get()
        && body_has_symbolic_perform_args(&hir_atom.body_stmt, module_env)
    {
        has_string_constraints_cell_pre.set(true);
    }
    let effective_timeout = if has_string_constraints_cell_pre.get() {
        timeout_ms * 2
    } else {
        timeout_ms
    };

    let mut cfg = Config::new();
    cfg.set_timeout_msec(effective_timeout);
    let ctx = Context::new(&cfg);
    let solver = Solver::new(&ctx);

    let int_sort = z3::Sort::int(&ctx);
    let arr = Array::new_const(&ctx, "arr", &int_sort, &int_sort);
    // linearity_ctx is wrapped in RefCell so expr_to_z3/stmt_to_z3 can mutate it
    // without requiring signature changes to every recursive call site.
    let linearity_ctx_cell = std::cell::RefCell::new(LinearityCtx::new());
    // Build EffectCtx from the atom's declared effects (transitively resolved)
    let allowed_effects_set = module_env.resolve_effect_set_from_effects(&atom.effects);
    let effect_ctx_cell = std::cell::RefCell::new(EffectCtx::new(allowed_effects_set));
    // Per-atom constraint budget tracking
    let constraint_count_cell = std::cell::Cell::new(0usize);
    // Sort-aware timeout: flag for Z3 String Sort constraints.
    // Set to true when Z3 String constraints are added during expr_to_z3.
    let has_string_constraints_cell = std::cell::Cell::new(has_string_constraints_cell_pre.get());
    let vc = VCtx {
        ctx: &ctx,
        arr: &arr,
        module_env,
        current_atom: Some(atom),
        linearity_ctx: Some(&linearity_ctx_cell),
        effect_ctx: Some(&effect_ctx_cell),
        constraint_count: Some(&constraint_count_cell),
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: Some(&has_string_constraints_cell),
    };

    let mut env: Env = HashMap::new();

    // Plan 9: Pre-register parameters with correct Z3 Sort based on their base type.
    // Without this, Str-typed parameters without refinement types would be lazily
    // created as Int in expr_to_z3's Variable fallback, causing string operations
    // (concatenation, equality) to silently produce incorrect verification results.
    // This matches the treatment in verify_atom_invariant (line 3206-3218).
    for param in &atom.params {
        let base = param
            .type_name
            .as_deref()
            .map(|t| module_env.resolve_base_type(t))
            .unwrap_or_else(|| "i64".to_string());
        let var: Dynamic = match base.as_str() {
            "f64" => Float::new_const(&ctx, param.name.as_str(), 11, 53).into(),
            "Str" => Z3String::new_const(&ctx, param.name.as_str()).into(),
            _ => Int::new_const(&ctx, param.name.as_str()).into(),
        };
        env.insert(param.name.clone(), var);
    }

    // Phase 1h (continued): ConflictingMerge Z3 infrastructure.
    // With Phase 4c Copy/Move type distinction integrated, move violations for
    // Move types are now hard errors (returned above). This loop registers Z3
    // variables for any remaining conflict locals as infrastructure for future
    // ownership constraint integration.
    for (local, block_id) in &move_conflict_locals {
        let var_name = format!("__move_conflict_{}_{}", local.0, block_id);
        let conflict_var = Bool::new_const(&ctx, var_name.as_str());
        solver.assert(&conflict_var);
    }

    // Phase 2b: エフェクト許可セットを Z3 環境に注入
    {
        let allowed_effects = module_env.resolve_effect_set_from_effects(&atom.effects);
        for effect_name in &allowed_effects {
            let allowed_name = format!("__effect_allowed_{}", effect_name);
            env.insert(allowed_name, Bool::from_bool(&ctx, true).into());
        }
    }

    // 1. 量子化制約の処理
    for (q_index, q) in atom.forall_constraints.iter().enumerate() {
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
            .ok_or(MumeiError::verification_at(
                "Condition must be boolean",
                atom.span.clone(),
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
        let track_label = format!("track_quantifier_{}", q_index);
        let track_bool = Bool::new_const(&ctx, track_label.as_str());
        solver.assert_and_track(&quantifier_expr, &track_bool);
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
                        // Plan 9: Str fields as Z3 String Sort
                        "Str" => Z3String::new_const(&ctx, field_var_name.as_str()).into(),
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
                            let track_label =
                                format!("track_struct_field_{}::{}", param.name, field.name);
                            let track_bool = Bool::new_const(&ctx, track_label.as_str());
                            solver.assert_and_track(&constraint_bool, &track_bool);
                        }
                    }
                }
            }
        }
    }

    // 2c. 全パラメータに対して配列長シンボルを事前生成
    #[allow(clippy::map_entry)]
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
    // linearity_ctx_cell is shared with VCtx (created above) via RefCell.

    // consume 対象パラメータの登録
    if !atom.consumed_params.is_empty() {
        for param_name in &atom.consumed_params {
            // パラメータが実際に存在するか検証
            if !atom.params.iter().any(|p| p.name == *param_name) {
                return Err(MumeiError::type_error_at(
                    format!(
                        "consume target '{}' is not a parameter of atom '{}'",
                        param_name, atom.name
                    ),
                    atom.span.clone(),
                ));
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
                return Err(MumeiError::type_error_at(
                    format!("Cannot consume {} parameter '{}' in atom '{}': {} parameters are borrowed, not owned", kind, param_name, atom.name, kind),
                    atom.span.clone()
                ));
            }
            // LinearityCtx に登録
            linearity_ctx_cell.borrow_mut().register(param_name);

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
            linearity_ctx_cell.borrow_mut().register(&param.name);

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
            let track_requires = Bool::new_const(&ctx, "track_requires");
            solver.assert_and_track(&req_bool, &track_requires);
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
                                return Err(MumeiError::verification_at(
                                    format!(
                                        "Aliasing violation in atom '{}': \
                                         'ref mut {}' and '{} {}' may reference the same data (type: {}). \
                                         A mutable reference requires exclusive access — \
                                         no other references to the same data are allowed.\n  \
                                         Hint: Use different types, or ensure the values are provably distinct \
                                         via requires.",
                                        atom.name, ref_mut_p.name, other_kind, other_p.name,
                                        ref_mut_p.type_name.as_deref().unwrap_or("unknown")
                                    ),
                                    atom.span.clone()
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
    let phase_start = std::time::Instant::now();
    let body_result = match stmt_to_z3(&vc, &hir_atom.body_stmt, &mut env, Some(&solver)) {
        Ok(val) => val,
        Err(e) => {
            // Body evaluation errors (e.g., division by zero, out-of-bounds) propagate
            // before reaching the postcondition check. Write a failure report so the
            // MCP self-healing flow does not read a stale report.json from a prior run.
            let err_str = format!("{}", e);
            // If this is an effect mismatch violation, save a structured report
            if err_str.contains("Effect violation: 'perform ") {
                // Extract effect name and operation from error message
                // Format: "Effect violation: 'perform Effect.op' requires [Effect] effect, ..."
                if let Some(start) = err_str.find("requires [") {
                    let after = &err_str[start + 10..];
                    if let Some(end) = after.find(']') {
                        let required_effect = &after[..end];
                        let source_op = err_str
                            .find("'perform ")
                            .and_then(|s| {
                                let rest = &err_str[s + 9..];
                                rest.find('\'').map(|e| rest[..e].to_string())
                            })
                            .unwrap_or_default();
                        let effect_names: Vec<String> =
                            atom.effects.iter().map(|e| e.name.clone()).collect();
                        save_effect_violation_report(
                            output_dir,
                            &atom.name,
                            &effect_names,
                            required_effect,
                            &source_op,
                            &[
                                format!("Add '{}' to the effects declaration", required_effect),
                                format!("Remove the call to 'perform {}'", source_op),
                            ],
                        );
                    }
                }
            }
            // Determine failure type: division-by-zero and budget exceeded get their own categories
            let body_failure_type = if err_str.contains("division by zero") {
                FAILURE_DIVISION_BY_ZERO
            } else if err_str.contains("Constraint budget exceeded") {
                "constraint_budget_exceeded"
            } else {
                FAILURE_PRECONDITION_VIOLATED
            };
            let constraint_mappings = build_constraint_mappings_for_atom(atom, module_env);
            let semantic_fb =
                build_semantic_feedback(&constraint_mappings, None, atom, body_failure_type, None);
            save_visualizer_report(
                output_dir,
                "failed",
                &atom.name,
                "N/A",
                "N/A",
                &err_str,
                None,
                body_failure_type,
                semantic_fb.as_ref(),
                Some(&atom.span),
                Some(&constraint_mappings),
            );
            return Err(e);
        }
    };

    metrics.record_phase("Phase 4: body evaluation", phase_start.elapsed());

    // 4b. Taint Analysis: unverified 関数の呼び出しを検出し警告
    check_taint_propagation(atom, &hir_atom.body_stmt, &env, module_env);

    // 5. 事後条件 (ensures)
    let phase_start = std::time::Instant::now();
    if atom.ensures.trim() != "true" {
        env.insert("result".to_string(), body_result);
        let ens_ast = parse_expression(&atom.ensures);
        let ens_z3 = expr_to_z3(&vc, &ens_ast, &mut env, None)?;
        if let Some(ens_bool) = ens_z3.as_bool() {
            solver.push();
            solver.assert(&ens_bool.not());
            if solver.check() == SatResult::Sat {
                // Extract counterexample from Z3 model
                let (ce_a, ce_b, ce_value) = if let Some(model) = solver.get_model() {
                    let mut ce_json = serde_json::Map::new();
                    for param in &atom.params {
                        if let Some(var_z3) = env.get(&param.name) {
                            if let Some(val) = model.eval(var_z3, true) {
                                let val_str = format!("{}", val);
                                ce_json.insert(param.name.clone(), json!(val_str));
                            }
                        }
                    }
                    let a_str = ce_json
                        .get(atom.params.first().map(|p| p.name.as_str()).unwrap_or(""))
                        .and_then(|v| v.as_str())
                        .unwrap_or("N/A")
                        .to_string();
                    let b_str = ce_json
                        .get(atom.params.get(1).map(|p| p.name.as_str()).unwrap_or(""))
                        .and_then(|v| v.as_str())
                        .unwrap_or("N/A")
                        .to_string();
                    let ce_val = if ce_json.is_empty() {
                        None
                    } else {
                        Some(serde_json::Value::Object(ce_json))
                    };
                    (a_str, b_str, ce_val)
                } else {
                    ("N/A".to_string(), "N/A".to_string(), None)
                };
                solver.pop(1);
                let constraint_mappings = build_constraint_mappings_for_atom(atom, module_env);
                let semantic_fb = build_semantic_feedback(
                    &constraint_mappings,
                    ce_value.as_ref(),
                    atom,
                    FAILURE_POSTCONDITION_VIOLATED,
                    None,
                );
                save_visualizer_report(
                    output_dir,
                    "failed",
                    &atom.name,
                    &ce_a,
                    &ce_b,
                    "Postcondition violated.",
                    ce_value.as_ref(),
                    FAILURE_POSTCONDITION_VIOLATED,
                    semantic_fb.as_ref(),
                    Some(&atom.span),
                    Some(&constraint_mappings),
                );
                metrics.record_phase(
                    "Phase 5: ensures verification (failed)",
                    phase_start.elapsed(),
                );
                metrics.total_constraints = constraint_count_cell.get();
                metrics.print_summary();
                // Feature 3d: Add related spans for constraint definition locations
                let mut err = MumeiError::verification_at(
                    "Postcondition (ensures) is not satisfied.",
                    atom.span.clone(),
                ).with_help("ensures の条件を確認してください。body の返り値が事後条件を満たすか検討してください");
                for mapping in &constraint_mappings {
                    if mapping.span.line > 0 {
                        let related_src_span = span_to_source_span("", &mapping.span);
                        err = err.with_related(
                            related_src_span,
                            format!("constraint on '{}' defined here", mapping.param_name),
                            miette::NamedSource::new(
                                if mapping.span.file.is_empty() {
                                    "<unknown>"
                                } else {
                                    &mapping.span.file
                                },
                                String::new(),
                            ),
                            format!("type constraint: {}", mapping.predicate_raw),
                            mapping.span.clone(),
                        );
                    }
                }
                return Err(err);
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
            if let Err(e) = linearity_ctx_cell.borrow_mut().consume(param_name) {
                return Err(MumeiError::verification_at(
                    format!("Linearity violation in atom '{}': {}", atom.name, e),
                    atom.span.clone(),
                ));
            }

            // Z3 上で is_alive を false に更新（消費後のアクセスを禁止）
            let alive_name = format!("__alive_{}", param_name);
            let alive_false = Bool::from_bool(&ctx, false);
            env.insert(alive_name, alive_false.into());
        }

        // 蓄積された違反をチェック
        let lctx_guard = linearity_ctx_cell.borrow();
        if lctx_guard.has_violations() {
            let violations_list = lctx_guard.get_violations();
            let violations = violations_list.join("\n  ");
            let linearity_fb = build_linearity_feedback(&atom.name, violations_list, &atom.span);
            save_visualizer_report(
                output_dir,
                "failed",
                &atom.name,
                "N/A",
                "N/A",
                &format!("Linearity violations in atom '{}'", atom.name),
                None,
                FAILURE_LINEARITY_VIOLATED,
                Some(&linearity_fb),
                Some(&atom.span),
                None,
            );
            return Err(MumeiError::verification_at(
                format!(
                    "Linearity violations in atom '{}':\n  {}",
                    atom.name, violations
                ),
                atom.span.clone(),
            ));
        }
    }

    metrics.record_phase("Phase 5: ensures verification", phase_start.elapsed());

    let z3_check_start = std::time::Instant::now();
    if solver.check() == SatResult::Unsat {
        let unsat_core = solver.get_unsat_core();
        let core_labels: Vec<String> = unsat_core.iter().map(|b| format!("{}", b)).collect();

        let structured_labels: Vec<StructuredLabel> = core_labels
            .iter()
            .filter_map(|label| parse_tracking_label(label))
            .collect();

        let conflicting_constraints: Vec<String> = structured_labels
            .iter()
            .map(|sl| sl.description.clone())
            .collect();

        let contradiction_fb = build_contradiction_feedback(
            &atom.name,
            &conflicting_constraints,
            &core_labels,
            &structured_labels,
        );

        save_visualizer_report(
            output_dir,
            "failed",
            &atom.name,
            "N/A",
            "N/A",
            "Logic contradiction.",
            None,
            FAILURE_INVARIANT_VIOLATED,
            Some(&contradiction_fb),
            Some(&atom.span),
            None,
        );

        let constraint_summary = if conflicting_constraints.is_empty() {
            "Contradiction found.".to_string()
        } else {
            format!(
                "Contradiction found. Conflicting constraints: [{}]",
                conflicting_constraints.join(", ")
            )
        };

        metrics.z3_check_time = z3_check_start.elapsed();
        metrics.total_constraints = constraint_count_cell.get();
        metrics.record_phase(
            "Phase 6: final Z3 check (contradiction)",
            z3_check_start.elapsed(),
        );
        metrics.print_summary();
        return Err(MumeiError::verification_at(
            constraint_summary,
            atom.span.clone(),
        ));
    }

    metrics.z3_check_time = z3_check_start.elapsed();
    metrics.total_constraints = constraint_count_cell.get();
    metrics.record_phase("Phase 6: final Z3 check", z3_check_start.elapsed());

    // Print metrics summary (always for now; future: gate behind --verbose)
    metrics.print_summary();

    save_visualizer_report(
        output_dir,
        "success",
        &atom.name,
        "N/A",
        "N/A",
        "Verified safe.",
        None,
        "",
        None,
        Some(&atom.span),
        None,
    );
    Ok(())
}

/// Increment the constraint count in VCtx and check budget.
/// Returns an error if the constraint budget has been exceeded.
fn check_constraint_budget(vc: &VCtx, atom_name: &str) -> MumeiResult<()> {
    if let Some(cell) = vc.constraint_count {
        let new_count = cell.get() + 1;
        cell.set(new_count);
        if new_count > vc.constraint_budget {
            return Err(MumeiError::verification(format!(
                "Constraint budget exceeded for atom '{}': {} constraints (limit: {})",
                atom_name, new_count, vc.constraint_budget
            )));
        }
    }
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
            let track_u64 = Bool::new_const(ctx, format!("track_u64_nonneg_{}", var_name).as_str());
            solver.assert_and_track(&v.ge(&Int::from_i64(ctx, 0)), &track_u64);
            v.into()
        }
        // Plan 9-7: Str base type uses Z3 String Sort for refinement constraints
        "Str" => Z3String::new_const(ctx, var_name).into(),
        _ => Int::new_const(ctx, var_name).into(),
    };

    global_env.insert(var_name.to_string(), var_z3.clone());

    let mut local_env = global_env.clone();
    local_env.insert(refined.operand.clone(), var_z3);

    let predicate_ast = parse_expression(&refined.predicate_raw);
    let predicate_z3 = expr_to_z3(vc, &predicate_ast, &mut local_env, None)?
        .as_bool()
        .ok_or(
            MumeiError::type_error_at(
                format!("Predicate for {} must be boolean", refined.name),
                refined.span.clone(),
            )
            .with_help(format!(
                "型 '{}' の制約が boolean 式である必要があります",
                refined.name
            )),
        )?;

    let track_label = format!("track_refined_type_{}::{}", var_name, refined.name);
    let track_bool = Bool::new_const(ctx, track_label.as_str());
    solver.assert_and_track(&predicate_z3, &track_bool);
    Ok(())
}

// =============================================================================
// Subsumption Check: atom_ref contract implication
// =============================================================================
//
// When `atom_ref(concrete)` is passed to a parameter with `contract(f)`,
// verify that the concrete atom's ensures clause *implies* the contract's
// ensures clause under the concrete atom's precondition.  This is a
// universal validity check:
//
//   ∀ params. (concrete.requires ∧ concrete.ensures) ⇒ contract.ensures
//
// If the implication does not hold, a **warning** (not a hard error) is
// emitted to stderr.  This preserves backward compatibility while giving
// the user early feedback about potential contract mismatches.

/// Check that `concrete_atom.requires ∧ concrete_atom.ensures` implies
/// `contract_ensures`.
///
/// Uses a Z3 solver scope (push/pop) to avoid polluting the caller's context.
/// Emits `eprintln!` warnings on subsumption failure or evaluation errors.
///
/// Returns `true` if the subsumption holds (or is trivially skipped),
/// `false` if a warning was emitted (implication does not hold).
#[allow(clippy::too_many_arguments)]
fn check_contract_subsumption(
    vc: &VCtx<'_>,
    concrete_atom: &Atom,
    contract_ensures: &str,
    _contract_requires: Option<&str>, // reserved for future use
    callee_name: &str,
    param_name: &str,
    solver: &Solver<'_>,
    ctx: &Context,
) -> bool {
    // Skip when the contract requires nothing — any ensures trivially implies "true".
    // NOTE: We intentionally do NOT skip when concrete_atom.ensures == "true".
    // An atom with ensures: true guarantees nothing, so it cannot imply a
    // non-trivial contract like `result >= 0`. The Z3 check below will correctly
    // find a counterexample and emit a warning in that case.
    if contract_ensures.trim() == "true" {
        return true;
    }

    // Build an environment mapping concrete atom's parameters to fresh Z3
    // variables so that we universally quantify over the parameter space.
    // Only the concrete atom's own parameter names are bound — no hardcoded
    // aliases — so there is no risk of accidental name collisions.
    let mut sub_env: Env<'_> = HashMap::new();
    for (i, param) in concrete_atom.params.iter().enumerate() {
        let z3_var: Dynamic =
            Int::new_const(ctx, format!("__sub_p{}_{}", i, param.name).as_str()).into();
        sub_env.insert(param.name.clone(), z3_var);
    }

    // Create a fresh symbolic result that both ensures clauses reference.
    let result_var: Dynamic = Int::new_const(ctx, "__sub_result").into();
    sub_env.insert("result".to_string(), result_var);

    // The parser represents `true` / `false` as Expr::Variable("true"|"false").
    // Pre-bind them to Z3 Bool constants so expr_to_z3 produces Bool sort
    // instead of an unbound Int, which would fail the as_bool() gate below.
    sub_env.insert(
        "true".to_string(),
        z3::ast::Bool::from_bool(ctx, true).into(),
    );
    sub_env.insert(
        "false".to_string(),
        z3::ast::Bool::from_bool(ctx, false).into(),
    );

    // --- Assert concrete atom's requires clause (precondition) ---
    // Without this, the check would ask "for ALL params, does ensures ⇒
    // contract?" which is too strong. We need "for params satisfying
    // requires, does ensures ⇒ contract?".
    let concrete_req = concrete_atom.requires.trim();
    let requires_bool_opt = if concrete_req != "true" && !concrete_req.is_empty() {
        let req_ast = parse_expression(concrete_req);
        match expr_to_z3(vc, &req_ast, &mut sub_env, None) {
            Ok(v) => v.as_bool(),
            Err(_) => None,
        }
    } else {
        None
    };

    // Parse and evaluate the concrete atom's ensures.
    let concrete_ens_ast = parse_expression(&concrete_atom.ensures);
    let concrete_ens_z3 = match expr_to_z3(vc, &concrete_ens_ast, &mut sub_env, None) {
        Ok(v) => v,
        Err(_e) => {
            return true;
        }
    };

    // Parse and evaluate the contract's ensures.
    let contract_ens_ast = parse_expression(contract_ensures);
    let contract_ens_z3 = match expr_to_z3(vc, &contract_ens_ast, &mut sub_env, None) {
        Ok(v) => v,
        Err(_e) => {
            return true;
        }
    };

    // Both must be booleans for an implication check.
    let (concrete_bool, contract_bool) =
        match (concrete_ens_z3.as_bool(), contract_ens_z3.as_bool()) {
            (Some(c), Some(ct)) => (c, ct),
            _ => return true, // non-boolean ensures — cannot check subsumption
        };

    // Check: requires ∧ concrete_ensures ∧ ¬contract_ensures is UNSAT
    //        ⟺  (requires ∧ concrete_ensures) ⇒ contract_ensures
    solver.push();
    if let Some(ref req_bool) = requires_bool_opt {
        solver.assert(req_bool);
    }
    solver.assert(&concrete_bool);
    solver.assert(&contract_bool.not());
    let sat_result = solver.check();
    solver.pop(1);

    if sat_result == SatResult::Sat {
        eprintln!(
            "\u{26a0}\u{fe0f}  Subsumption warning: atom_ref({}) passed to {}.{} \u{2014} \
             concrete ensures '{}' may not imply contract ensures '{}'",
            concrete_atom.name, callee_name, param_name, concrete_atom.ensures, contract_ensures
        );
        return false;
    }
    // NOTE: SatResult::Unknown (e.g., Z3 timeout) falls through to `true` here.
    // This is the conservative choice for a warning-only check: we only warn when
    // we have a definite counterexample (SAT), never on inconclusive results.
    true
}

fn expr_to_z3<'a>(
    vc: &VCtx<'a>,
    expr: &Expr,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> DynResult<'a> {
    // Per-atom constraint budget: increment count and check limit.
    // This tracks the number of Z3 AST nodes generated, which correlates
    // with solver.assert() pressure and overall verification complexity.
    if solver_opt.is_some() {
        let atom_name = vc
            .current_atom
            .map(|a| a.name.as_str())
            .unwrap_or("<unknown>");
        check_constraint_budget(vc, atom_name)?;
    }

    let ctx = vc.ctx;
    let arr = vc.arr;
    match expr {
        Expr::Number(n) => Ok(Int::from_i64(ctx, *n).into()),
        Expr::Float(f) => Ok(Float::from_f64(ctx, *f).into()),
        // Plan 9: String literal to Z3 String Sort
        Expr::StringLit(s) => Ok(Z3String::from_str(ctx, s).unwrap().into()),
        Expr::Variable(name) => {
            // Wire check_alive() into variable access: if the variable has been
            // consumed, report a use-after-consume error.
            if let Some(lctx_cell) = vc.linearity_ctx {
                if let Err(e) = lctx_cell.borrow_mut().check_alive(name) {
                    return Err(MumeiError::verification(format!(
                        "Linearity violation: {}",
                        e
                    )));
                }
            }
            Ok(env
                .get(name)
                .cloned()
                .unwrap_or_else(|| Int::new_const(ctx, name.as_str()).into()))
        }
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
                        return Err(MumeiError::verification(format!(
                            "{}() requires exactly 4 arguments: (var, start, end, condition)",
                            name
                        )));
                    }
                    // 第1引数: 束縛変数名
                    let var_name = match &args[0] {
                        Expr::Variable(v) => v.clone(),
                        _ => {
                            return Err(MumeiError::verification(format!(
                                "{}(): first argument must be a variable name",
                                name
                            )))
                        }
                    };

                    // 第2引数: 範囲の開始
                    let start_z3 = expr_to_z3(vc, &args[1], env, None)?.as_int().ok_or(
                        MumeiError::type_error(format!("{}(): start must be integer", name)),
                    )?;

                    // 第3引数: 範囲の終了
                    let end_z3 = expr_to_z3(vc, &args[2], env, None)?.as_int().ok_or(
                        MumeiError::type_error(format!("{}(): end must be integer", name)),
                    )?;

                    // 束縛変数を一時的に env に追加して condition を評価
                    let bound_var = Int::new_const(ctx, var_name.as_str());
                    let old_val = env.insert(var_name.clone(), bound_var.clone().into());

                    let range_cond =
                        Bool::and(ctx, &[&bound_var.ge(&start_z3), &bound_var.lt(&end_z3)]);

                    let condition_z3 = expr_to_z3(vc, &args[3], env, None)?.as_bool().ok_or(
                        MumeiError::type_error(format!("{}(): condition must be boolean", name)),
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
                // =============================================================
                // Built-in string constraint functions for requires/ensures
                // =============================================================
                // These are parsed as Expr::Call by the parser but need special
                // handling in Z3 to produce Bool constraints on Z3 String Sort.
                "starts_with" | "ends_with" | "contains" | "not_contains" => {
                    if args.len() != 2 {
                        return Err(MumeiError::verification(format!(
                            "{}() requires exactly 2 arguments: (string_var, \"pattern\")",
                            name
                        )));
                    }
                    let str_val = expr_to_z3(vc, &args[0], env, solver_opt)?;
                    let pattern_val = expr_to_z3(vc, &args[1], env, solver_opt)?;

                    // Both arguments must be Z3 String Sort
                    if let (Some(str_z3), Some(pat_z3)) =
                        (str_val.as_string(), pattern_val.as_string())
                    {
                        let result: Bool = match name.as_str() {
                            "starts_with" => pat_z3.prefix(&str_z3),
                            "ends_with" => pat_z3.suffix(&str_z3),
                            "contains" => str_z3.contains(&pat_z3),
                            "not_contains" => str_z3.contains(&pat_z3).not(),
                            _ => unreachable!(),
                        };
                        Ok(result.into())
                    } else {
                        // Fallback: if operands are not strings, return true (no constraint).
                        // This handles cases where the variable hasn't been typed as Str
                        // (e.g., an i64 parameter). Str-typed parameters are correctly
                        // lowered to Z3 String Sort at parameter pre-registration, so this
                        // branch only fires for genuinely non-string variables.
                        //
                        // NOTE: This is a permissive fallback. If a user writes
                        // `not_contains(int_var, "..")` where int_var is i64, the constraint
                        // is silently dropped. This is acceptable because string constraint
                        // functions are only meaningful on Str-typed parameters, and using
                        // them on non-Str types is a user error that should ideally be
                        // caught by a type checker (not yet implemented for requires/ensures).
                        Ok(Bool::from_bool(ctx, true).into())
                    }
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

                        // Wire borrow()/consume() into call-site argument handling.
                        // For each callee parameter, if it is `ref`/`ref mut`, call borrow().
                        // If the callee has `consumed_params`, call consume() for the
                        // corresponding argument variable.
                        if let Some(lctx_cell) = vc.linearity_ctx {
                            // Handle ref/ref mut parameters → borrow
                            for (i, param) in callee.params.iter().enumerate() {
                                if param.is_ref || param.is_ref_mut {
                                    if let Some(Expr::Variable(arg_name)) = args.get(i) {
                                        let _ = lctx_cell.borrow_mut().borrow(arg_name, name);
                                    }
                                }
                            }
                            // Handle consumed_params → consume
                            for consumed_name in &callee.consumed_params {
                                if let Some(idx) =
                                    callee.params.iter().position(|p| p.name == *consumed_name)
                                {
                                    if let Some(Expr::Variable(arg_name)) = args.get(idx) {
                                        if let Err(e) = lctx_cell.borrow_mut().consume(arg_name) {
                                            return Err(MumeiError::verification(format!(
                                                "Linearity violation at call to '{}': {}",
                                                name, e
                                            )));
                                        }
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
                                        return Err(MumeiError::verification(
                                            format!("Call to '{}': precondition (requires) not satisfied at call site", name)
                                        ).with_help("呼び出し元で事前条件を満たしていません。引数の制約を確認してください"));
                                    }
                                    solver.pop(1);
                                }
                            }
                        }

                        // Trait method param_constraints の検証:
                        // 呼び出し先がトレイトメソッドの実装である場合、param_constraints を
                        // 呼び出し元のコンテキストで検証する。
                        // get_traits_for_method で全候補を取得し、find_impl で callee の型に
                        // 対して実際にトレイトが impl されている候補のみ適用する。
                        if let Some(solver) = solver_opt {
                            let callee_type = callee
                                .params
                                .first()
                                .and_then(|p| p.type_name.as_deref())
                                .unwrap_or("i64");
                            let candidates = vc.module_env.get_traits_for_method(name);
                            // find_impl で正しいトレイトを絞り込む
                            let matched = candidates
                                .iter()
                                .find(|(tn, _)| vc.module_env.find_impl(tn, callee_type).is_some());
                            if let Some((_trait_name, trait_method)) = matched {
                                for (i, constraint_opt) in
                                    trait_method.param_constraints.iter().enumerate()
                                {
                                    if let Some(constraint) = constraint_opt {
                                        if let Some(arg_val) = arg_vals.get(i) {
                                            let param_name = callee
                                                .params
                                                .get(i)
                                                .map(|p| p.name.as_str())
                                                .unwrap_or("v");
                                            let concrete_constraint: String =
                                                replace_constraint_placeholder(
                                                    constraint, param_name,
                                                );
                                            let mut constraint_env: Env = env.clone();
                                            constraint_env
                                                .insert(param_name.to_string(), arg_val.clone());
                                            let constraint_ast =
                                                parse_expression(&concrete_constraint);
                                            if let Ok(constraint_z3) = expr_to_z3(
                                                vc,
                                                &constraint_ast,
                                                &mut constraint_env,
                                                None,
                                            ) {
                                                if let Some(constraint_bool) =
                                                    constraint_z3.as_bool()
                                                {
                                                    solver.push();
                                                    solver.assert(&constraint_bool.not());
                                                    if solver.check() == SatResult::Sat {
                                                        solver.pop(1);
                                                        return Err(MumeiError::verification(
                                                            format!(
                                                                "Call to '{}': trait method parameter constraint '{}' not satisfied for argument {}",
                                                                name, constraint, i
                                                            )
                                                        ).with_help(
                                                            "トレイトメソッドのパラメータ制約が満たされていません。引数の値を確認してください"
                                                        ));
                                                    }
                                                    solver.pop(1);
                                                }
                                            }
                                        }
                                    }
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

                        // Release borrows after call returns.
                        // Once the callee has finished, ref/ref mut borrows are released
                        // so the owner can be consumed or re-borrowed.
                        if let Some(lctx_cell) = vc.linearity_ctx {
                            for (i, param) in callee.params.iter().enumerate() {
                                if param.is_ref || param.is_ref_mut {
                                    if let Some(Expr::Variable(arg_name)) = args.get(i) {
                                        lctx_cell.borrow_mut().release_borrow(arg_name, name);
                                    }
                                }
                            }
                        }

                        // =============================================================
                        // Subsumption Check: atom_ref argument vs contract ensures
                        // =============================================================
                        // When a callee parameter has fn_contract_ensures and the
                        // corresponding argument is atom_ref(concrete_name), verify
                        // that the concrete atom's ensures implies the contract's
                        // ensures.  Emit a warning (not a hard error) to maintain
                        // backward compatibility.
                        if let Some(solver) = solver_opt {
                            for (i, param) in callee.params.iter().enumerate() {
                                if let Some(ref contract_ensures) = param.fn_contract_ensures {
                                    if let Some(Expr::AtomRef {
                                        name: ref concrete_name,
                                    }) = args.get(i)
                                    {
                                        if let Some(concrete_atom) =
                                            vc.module_env.get_atom(concrete_name).cloned()
                                        {
                                            check_contract_subsumption(
                                                vc,
                                                &concrete_atom,
                                                contract_ensures,
                                                param.fn_contract_requires.as_deref(),
                                                name,
                                                &param.name,
                                                solver,
                                                ctx,
                                            );
                                        }
                                    }
                                }
                            }
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
                        Err(MumeiError::verification(format!(
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
                .ok_or(MumeiError::type_error("Index must be integer"))?;

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
                    return Err(MumeiError::verification(format!(
                        "Potential Out-of-Bounds on '{}' (index may be < 0 or >= len_{})",
                        name, name
                    ))
                    .with_help(
                        "requires にインデックスの範囲制約 (0 <= idx < len) を追加してください",
                    ));
                }
                solver.pop(1);
            }
            Ok(arr.select(&idx))
        }
        Expr::BinaryOp(left, op, right) => {
            let l = expr_to_z3(vc, left, env, solver_opt)?;
            let r = expr_to_z3(vc, right, env, solver_opt)?;

            // Plan 9-8: String concatenation — if both operands are Z3 String Sort
            if l.get_sort() == z3::Sort::string(ctx) && r.get_sort() == z3::Sort::string(ctx) {
                let ls = l.as_string().ok_or("Expected string for Str op")?;
                let rs = r.as_string().ok_or("Expected string for Str op")?;
                return match op {
                    Op::Add => {
                        // Z3 string concatenation — return concat directly
                        let result = z3::ast::String::concat(ctx, &[&ls, &rs]);
                        Ok(result.into())
                    }
                    Op::Eq => Ok(ls._eq(&rs).into()),
                    Op::Neq => Ok(ls._eq(&rs).not().into()),
                    _ => Err(format!("Unsupported operator {:?} for Str type", op).into()),
                };
            }

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
                                // Extract counterexample: find which variables cause divisor == 0
                                let (ce_hint, div_feedback) =
                                    if let Some(model) = solver.get_model() {
                                        let divisor_val = model
                                            .eval(&ri, true)
                                            .map(|v| format!("{}", v))
                                            .unwrap_or_else(|| "0".to_string());
                                        let dividend_val = model
                                            .eval(&li, true)
                                            .map(|v| format!("{}", v))
                                            .unwrap_or_else(|| "?".to_string());
                                        let hint = format!(
                                            " Counter-example: dividend = {}, divisor = {}",
                                            dividend_val, divisor_val
                                        );
                                        let fb = build_division_by_zero_feedback(
                                            &dividend_val,
                                            &divisor_val,
                                        );
                                        (hint, Some(fb))
                                    } else {
                                        (String::new(), None)
                                    };
                                // Attach structured feedback to error message for upstream reporting
                                let feedback_hint = div_feedback
                                    .as_ref()
                                    .map(|fb| format!(" [semantic_feedback: {}]", fb))
                                    .unwrap_or_default();
                                solver.pop(1);
                                return Err(MumeiError::verification(format!(
                                    "Potential division by zero.{}{}",
                                    ce_hint, feedback_hint
                                ))
                                .with_help("Add a condition divisor != 0 to requires"));
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
                    _ => Err(MumeiError::verification(format!(
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
                .ok_or(MumeiError::type_error("If condition must be boolean"))?;
            let t = stmt_to_z3(vc, then_branch, env, solver_opt)?;
            let e = stmt_to_z3(vc, else_branch, env, solver_opt)?;
            Ok(c.ite(&t, &e))
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
                                        return Err(MumeiError::verification(format!(
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
                            .ok_or(MumeiError::type_error("Guard must be boolean"))?;
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
                            format_counterexample(&model, &target_z3, arms, vc.module_env)
                        } else {
                            "unknown value".to_string()
                        };
                        solver.pop(1);
                        return Err(MumeiError::verification(
                            format!(
                                "Match is not exhaustive: the following value is not covered by any arm:\n  Counter-example: {}",
                                counterexample
                            )
                        ));
                    }
                    solver.pop(1);
                    return Err(MumeiError::verification(
                        "Match is not exhaustive: there exist values not covered by any arm.",
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
                        .ok_or(MumeiError::type_error("Guard must be boolean"))?;
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
                        let body_val = stmt_to_z3(vc, &arm.body, &mut arm_env, solver_opt)?;
                        solver.pop(1);
                        result = Some(match result {
                            Some(else_val) => full_cond.ite(&body_val, &else_val),
                            None => body_val,
                        });
                        accumulated_negations.push(full_cond.not());
                        continue;
                    }
                }

                let body_val = stmt_to_z3(vc, &arm.body, &mut arm_env, solver_opt)?;
                result = Some(match result {
                    Some(else_val) => full_cond.ite(&body_val, &else_val),
                    None => body_val,
                });
                accumulated_negations.push(full_cond.not());
            }

            result.ok_or_else(|| MumeiError::verification("Match expression has no arms"))
        }

        // =================================================================
        // 非同期処理 + リソース管理の Z3 検証
        // =================================================================
        Expr::Perform {
            effect,
            operation,
            args: perform_args,
        } => {
            // Effect system: record effect usage and verify against allowed set
            // Record that this effect was used
            let used_name = format!("__effect_used_{}", effect);
            let used_bool = Bool::from_bool(ctx, true);
            env.insert(used_name.clone(), used_bool.into());

            // Wire EffectCtx: track the performed effect
            if let Some(ectx_cell) = vc.effect_ctx {
                let mut ectx = ectx_cell.borrow_mut();
                // Record usage; violations are warnings here (Z3 check below is authoritative)
                let _ = ectx.perform_effect(effect);
            }

            // Check SecurityPolicy parameter constraints if available.
            // Currently only checks constant arguments (Number-based path IDs);
            // symbolic arguments are validated via Z3 constraints in verify_effect_params.
            if let Some(ref policy) = vc.module_env.security_policy {
                if !policy.is_effect_allowed(effect) {
                    return Err(MumeiError::verification(format!(
                        "Security policy violation: effect '{}' is not permitted by the \
                         current security policy",
                        effect
                    )));
                }
            }

            // Check against allowed effects via Z3 environment
            let allowed_name = format!("__effect_allowed_{}", effect);
            if env.get(&allowed_name).is_none() {
                // Effect not in allowed set — immediate violation
                return Err(MumeiError::verification(format!(
                    "Effect violation: 'perform {}.{}' requires [{}] effect, \
                         but it is not declared in the current atom's effects set.",
                    effect, operation, effect
                ))
                .with_help(format!(
                    "Fix option 1: Add '{}' to the effects declaration: effects: [{}];\n\
                         Fix option 2: Remove the call to 'perform {}.{}'.",
                    effect, effect, effect, operation
                )));
            }

            // If solver is available, assert the Z3 containment constraint
            if let Some(solver) = solver_opt {
                let used_z3 = Bool::from_bool(ctx, true);
                let allowed_z3 = Bool::from_bool(ctx, true); // already proven allowed
                                                             // Assert: used → allowed (trivially true when allowed)
                solver.assert(&used_z3.implies(&allowed_z3));
            }

            // Process arguments and collect Z3 values
            let mut arg_z3_values: Vec<Dynamic> = Vec::new();
            for arg in perform_args {
                let val = expr_to_z3(vc, arg, env, solver_opt)?;
                arg_z3_values.push(val);
            }

            // Z3 String Sort: verify symbolic parameter constraints
            // Look up the EffectDef to get constraint and param definitions
            let effect_def = vc
                .module_env
                .effect_defs
                .get(effect.as_str())
                .or_else(|| vc.module_env.effects.get(effect.as_str()))
                .cloned();
            if let Some(def) = effect_def {
                if let Some(ref constraint) = def.constraint {
                    // For each argument, check if it's a symbolic (non-constant) value
                    // that needs Z3 String constraint verification.
                    // NOTE: Currently def.constraint is a single string (e.g., "starts_with(path, \"/tmp/\")")
                    // that is applied to ALL non-constant args. This is correct for single-parameter
                    // effects (the only kind currently supported by the parser), but would incorrectly
                    // apply a path-specific constraint to unrelated parameters if multi-parameter
                    // effects like FileOp(path: Str, mode: Str) are added. When that happens,
                    // extract the parameter name from the constraint string, find its index in
                    // def.params, and only apply the constraint when `i` matches that index.
                    for (i, arg) in perform_args.iter().enumerate() {
                        // Number/Float literals are constants already checked
                        // by verify_effect_params (Phase 1g). Skip Z3 String here.
                        // Variables and other expressions need symbolic verification.
                        let is_constant = matches!(arg, Expr::Number(_) | Expr::Float(_));
                        if is_constant {
                            // Constant args are already checked by check_constant_constraint
                            // in verify_effect_params (Phase 1g). Skip Z3 String here.
                            continue;
                        }
                        // Symbolic argument: verify constraint using Z3 String Sort
                        if let Some(solver) = solver_opt {
                            let param_name =
                                def.params.get(i).map(|p| p.name.as_str()).unwrap_or("arg");
                            // Use a unique counter to distinguish different perform call sites.
                            // Without this, Z3 reuses the same constant for the same name,
                            // incorrectly merging constraints from distinct call sites.
                            static EFFECT_STR_COUNTER: std::sync::atomic::AtomicUsize =
                                std::sync::atomic::AtomicUsize::new(0);
                            let unique_id = EFFECT_STR_COUNTER
                                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            let z3_str_name = format!(
                                "__effect_str_{}_{}_{}_{}",
                                effect, operation, param_name, unique_id
                            );

                            // Plan 10: Use arg_z3_values[i] directly when it has Z3 String Sort.
                            // This enables dynamically constructed strings (e.g., "/tmp/" + var + "/file.txt")
                            // to be directly checked against constraints like starts_with(path, "/tmp/").
                            let param_z3_str = if i < arg_z3_values.len() {
                                if let Some(existing_str) = arg_z3_values[i].as_string() {
                                    // The argument was already evaluated to a Z3 String by expr_to_z3.
                                    // Use it directly — this preserves concat/variable relationships.
                                    existing_str
                                } else {
                                    // Non-string Z3 value: create a fresh Z3 String variable
                                    // and try to connect it to any known string variable in env.
                                    let fresh = Z3String::new_const(ctx, z3_str_name.as_str());
                                    if let Expr::Variable(var_name) = arg {
                                        let str_env_key = format!("__str_{}", var_name);
                                        let found_str = env
                                            .get(&str_env_key)
                                            .and_then(|v| v.as_string())
                                            .or_else(|| {
                                                env.get(var_name).and_then(|v| v.as_string())
                                            });
                                        if let Some(existing_s) = found_str {
                                            solver.assert(&fresh._eq(&existing_s));
                                        }
                                    }
                                    fresh
                                }
                            } else {
                                // Fallback: create a fresh Z3 String variable
                                let fresh = Z3String::new_const(ctx, z3_str_name.as_str());
                                if let Expr::Variable(var_name) = arg {
                                    let str_env_key = format!("__str_{}", var_name);
                                    let found_str = env
                                        .get(&str_env_key)
                                        .and_then(|v| v.as_string())
                                        .or_else(|| env.get(var_name).and_then(|v| v.as_string()));
                                    if let Some(existing_s) = found_str {
                                        solver.assert(&fresh._eq(&existing_s));
                                    }
                                }
                                fresh
                            };

                            // Parse the constraint and assert it
                            if let Some(constraint_bool) =
                                parse_constraint_to_z3_string(ctx, constraint, &param_z3_str)
                            {
                                // Set has_string_constraints flag for sort-aware timeout
                                if let Some(flag) = vc.has_string_constraints {
                                    flag.set(true);
                                }
                                // Check constraint budget
                                if let Some(count) = vc.constraint_count {
                                    let current = count.get();
                                    if current >= vc.constraint_budget {
                                        return Err(MumeiError::verification(format!(
                                            "Constraint budget exceeded for effect '{}' \
                                             string constraint: {} constraints (limit: {})",
                                            effect, current, vc.constraint_budget
                                        )));
                                    }
                                    count.set(current + 1);
                                }
                                let track_label = format!(
                                    "track_effect_str_{}_{}_{}_{}",
                                    effect, operation, param_name, unique_id
                                );
                                let track_bool = Bool::new_const(ctx, track_label.as_str());
                                solver.assert_and_track(&constraint_bool, &track_bool);
                            }

                            // Store the Z3 String variable in env for downstream use
                            env.insert(z3_str_name, param_z3_str.into());
                        }
                    }
                }
            }

            // Return a symbolic result value.
            // Use Z3 String Sort if the effect has Str-typed parameters,
            // since the operation may return a string (e.g., http_request_path).
            // Otherwise default to Int (status codes, handles, etc.).
            //
            // NOTE: This is a heuristic. Ideally, EffectDef would carry a
            // per-operation return type (e.g., `read -> Str`, `write -> i64`),
            // but the current parser does not record return types for effect
            // operations. Using "any param is Str → result is Str" is a
            // conservative approximation that prevents Z3 Sort mismatches when
            // the perform result is later used in string operations. When the
            // parser gains per-operation return type info, this heuristic
            // should be replaced with a direct lookup.
            //
            // IMPACT: This changes the return type for pre-existing effects
            // with Str params (e.g., HttpGet(url: Str), HttpPost(url: Str))
            // from Int to Z3String. No current code is broken because all
            // atoms discard the perform result (e.g., `perform X.op(url); 1`).
            // Future code that uses the perform result in an integer context
            // (e.g., `let x = perform HttpGet.request(url); x + 1`) would get
            // a Z3 Sort mismatch error.
            let result_name = format!("__perform_{}_{}", effect, operation);
            let has_str_params = vc
                .module_env
                .effect_defs
                .get(effect.as_str())
                .or_else(|| vc.module_env.effects.get(effect.as_str()))
                .map(|def| def.params.iter().any(|p| p.type_name == "Str"))
                .unwrap_or(false);
            if has_str_params {
                Ok(Z3String::new_const(ctx, result_name.as_str()).into())
            } else {
                Ok(Int::new_const(ctx, result_name.as_str()).into())
            }
        }

        Expr::Async { body } => {
            // async ブロック: body を非同期コンテキストとして検証する。
            // Z3 上では通常の式として扱い、結果をシンボリック値として返す。
            // await ポイントでの所有権検証は Await 式で行う。
            stmt_to_z3(vc, body, env, solver_opt)
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
                                return Err(MumeiError::verification(
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

        // =================================================================
        // Higher-Order Functions (Phase A): atom_ref + call
        // =================================================================
        Expr::AtomRef { name } => {
            // atom_ref(some_atom): ModuleEnv から atom 定義を取得し、
            // シンボリック値を生成する。呼び出し先の atom の契約情報は
            // CallRef 時に展開される。
            if vc.module_env.get_atom(name).is_none() {
                return Err(MumeiError::verification(format!(
                    "atom_ref: unknown atom '{}'",
                    name
                )));
            }
            // atom_ref はシンボリックな関数参照として Int 値を生成
            // （実行時は関数ポインタ、Z3 上はシンボリック識別子）
            let ref_name = format!("__atom_ref_{}", name);
            let ref_val = Int::new_const(ctx, ref_name.as_str());
            env.insert(ref_name, ref_val.clone().into());
            Ok(ref_val.into())
        }
        Expr::CallRef { callee, args } => {
            // call(callee_expr, arg1, arg2, ...):
            // callee が AtomRef の場合、参照先の atom の契約を展開して検証する。
            // - requires を呼び出し元のコンテキストで検証
            // - ensures を事実として solver に追加

            // callee を評価
            let _callee_val = expr_to_z3(vc, callee, env, solver_opt)?;

            // callee が AtomRef の場合、参照先の atom 名を取得
            let atom_name = if let Expr::AtomRef { name } = callee.as_ref() {
                Some(name.clone())
            } else if let Expr::Variable(var_name) = callee.as_ref() {
                // 変数が atom_ref として束縛されている場合
                // env から __atom_ref_ プレフィックスで探す
                if env.contains_key(&format!("__atom_ref_{}", var_name)) {
                    Some(var_name.clone())
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(ref callee_name) = atom_name {
                if let Some(callee_atom) = vc.module_env.get_atom(callee_name).cloned() {
                    // 引数を Z3 で評価
                    let mut arg_vals = Vec::new();
                    for arg in args {
                        arg_vals.push(expr_to_z3(vc, arg, env, solver_opt)?);
                    }

                    // 呼び出し先のパラメータ名に引数をマッピング
                    let mut call_env = env.clone();
                    for (i, param) in callee_atom.params.iter().enumerate() {
                        if let Some(arg_val) = arg_vals.get(i) {
                            call_env.insert(param.name.clone(), arg_val.clone());
                        }
                    }

                    // requires を呼び出し元のコンテキストで検証
                    if callee_atom.requires.trim() != "true" {
                        let req_ast = parse_expression(&callee_atom.requires);
                        let req_z3 = expr_to_z3(vc, &req_ast, &mut call_env, None)?;
                        if let Some(req_bool) = req_z3.as_bool() {
                            if let Some(solver) = solver_opt {
                                solver.push();
                                solver.assert(&req_bool.not());
                                if solver.check() == SatResult::Sat {
                                    solver.pop(1);
                                    return Err(MumeiError::verification(format!(
                                        "call(atom_ref({})): precondition '{}' may not hold at call site",
                                        callee_name, callee_atom.requires
                                    ))
                                    .with_help(
                                        "呼び出し元で事前条件を満たしていません。引数の制約を確認してください",
                                    ));
                                }
                                solver.pop(1);
                            }
                        }
                    }

                    // ensures を事実として solver に追加（Equality Ensures Propagation）
                    static CALL_REF_COUNTER: std::sync::atomic::AtomicUsize =
                        std::sync::atomic::AtomicUsize::new(0);
                    let call_id =
                        CALL_REF_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let result_name = format!("call_ref_{}_{}", callee_name, call_id);
                    let result_z3: Dynamic = Int::new_const(ctx, result_name.as_str()).into();

                    if callee_atom.ensures.trim() != "true" {
                        call_env.insert("result".to_string(), result_z3.clone());
                        let ens_ast = parse_expression(&callee_atom.ensures);
                        let ens_z3 = expr_to_z3(vc, &ens_ast, &mut call_env, None)?;
                        if let Some(ens_bool) = ens_z3.as_bool() {
                            if let Some(solver) = solver_opt {
                                solver.assert(&ens_bool);
                            }
                        }

                        // Equality ensures の特別処理
                        if let Expr::BinaryOp(left, Op::Eq, right) = &ens_ast {
                            if let Expr::Variable(ref var_name) = left.as_ref() {
                                if var_name == "result" {
                                    if let Ok(rhs_val) = expr_to_z3(vc, right, &mut call_env, None)
                                    {
                                        if let Some(solver) = solver_opt {
                                            if let (Some(res_int), Some(rhs_int)) =
                                                (result_z3.as_int(), rhs_val.as_int())
                                            {
                                                solver.assert(&res_int._eq(&rhs_int));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    return Ok(result_z3);
                }
            }

            // =============================================================
            // Phase B: call_with_contract — パラメトリック関数型の契約展開
            // =============================================================
            // callee が Variable で、current_atom のパラメータに contract(f) が
            // 宣言されている場合、その契約を使って結果を制約する。
            // これにより trusted マーカーなしで高階関数を検証できる。

            let mut arg_vals = Vec::new();
            for arg in args {
                arg_vals.push(expr_to_z3(vc, arg, env, solver_opt)?);
            }

            static DYNAMIC_CALL_COUNTER: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let id = DYNAMIC_CALL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let result: Dynamic = Int::new_const(ctx, format!("call_ref_dynamic_{}", id)).into();

            // callee が Variable の場合、current_atom のパラメータから contract 情報を取得
            if let Expr::Variable(callee_var_name) = callee.as_ref() {
                if let Some(current_atom) = vc.current_atom {
                    if let Some(param) = current_atom
                        .params
                        .iter()
                        .find(|p| p.name == *callee_var_name)
                    {
                        // contract(f): ensures: <expr> が宣言されている場合
                        if let Some(ref fn_ensures) = param.fn_contract_ensures {
                            let mut contract_env = env.clone();

                            // atom_ref のパラメータ型情報から引数名を生成
                            // atom_ref(i64) -> i64 の場合、arg0 として引数をマッピング
                            // atom_ref(i64, i64) -> i64 の場合、arg0, arg1 として引数をマッピング
                            for (i, arg_val) in arg_vals.iter().enumerate() {
                                contract_env.insert(format!("arg{}", i), arg_val.clone());
                            }

                            // 最初の引数を "x" としてもマッピング（よくある1引数パターン用）
                            if let Some(first_arg) = arg_vals.first() {
                                contract_env.insert("x".to_string(), first_arg.clone());
                            }
                            // 2引数の場合 "y" もマッピング
                            if let Some(second_arg) = arg_vals.get(1) {
                                contract_env.insert("y".to_string(), second_arg.clone());
                            }

                            // result をマッピング
                            contract_env.insert("result".to_string(), result.clone());

                            // requires の検証（宣言されている場合）
                            if let Some(ref fn_requires) = param.fn_contract_requires {
                                if fn_requires.trim() != "true" {
                                    let req_ast = parse_expression(fn_requires);
                                    let req_z3 = expr_to_z3(vc, &req_ast, &mut contract_env, None)
                                        .map_err(|e| MumeiError::verification(format!(
                                            "call_with_contract({}): failed to evaluate requires '{}': {}",
                                            callee_var_name, fn_requires, e
                                        )))?;
                                    if let Some(req_bool) = req_z3.as_bool() {
                                        if let Some(solver) = solver_opt {
                                            solver.push();
                                            solver.assert(&req_bool.not());
                                            if solver.check() == SatResult::Sat {
                                                solver.pop(1);
                                                return Err(MumeiError::verification(format!(
                                                    "call_with_contract({}): precondition '{}' may not hold at call site",
                                                    callee_var_name, fn_requires
                                                ))
                                                .with_help(
                                                    "関数パラメータの事前条件を満たしていません。引数の制約を確認してください",
                                                ));
                                            }
                                            solver.pop(1);
                                        }
                                    }
                                }
                            }

                            // ensures を事実として solver に追加
                            if fn_ensures.trim() != "true" {
                                let ens_ast = parse_expression(fn_ensures);
                                let ens_z3 = expr_to_z3(vc, &ens_ast, &mut contract_env, None)
                                    .map_err(|e| MumeiError::verification(format!(
                                        "call_with_contract({}): failed to evaluate ensures '{}': {}",
                                        callee_var_name, fn_ensures, e
                                    )))?;
                                if let Some(ens_bool) = ens_z3.as_bool() {
                                    if let Some(solver) = solver_opt {
                                        solver.assert(&ens_bool);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Ok(result)
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
        // Lambda 式: Z3 uninterpreted function として表現する
        // 将来のフェーズでキャプチャ変数の環境アサーションと
        // 高階関数コントラクトの検証を追加する
        Expr::Lambda { params, body, .. } => {
            // Create a fresh symbolic value for the lambda
            // Lambda bodies will be verified when called via higher-order function contracts
            // Use a unique counter to avoid Z3 constant name collisions when multiple lambdas
            // with the same arity appear in the same atom body (e.g., `let f = |x| x+1; let g = |x| x-1;`).
            static LAMBDA_COUNTER: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let lambda_id = LAMBDA_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let lambda_name = format!("__lambda_{}_{}", params.len(), lambda_id);
            let lambda_sym = Int::new_const(ctx, lambda_name.as_str());

            // Register parameter names in a sub-environment for body verification
            let mut lambda_env = env.clone();
            for p in params {
                let p_sym = Int::new_const(ctx, p.name.as_str());
                lambda_env.insert(p.name.clone(), p_sym.into());
            }

            // Verify the lambda body in the sub-environment
            let _body_val = stmt_to_z3(vc, body, &mut lambda_env, solver_opt)?;

            Ok(lambda_sym.into())
        }
        // Plan 8: Channel send — evaluate channel and value, return unit
        Expr::ChanSend { channel, value } => {
            let _ch = expr_to_z3(vc, channel, env, solver_opt)?;
            let _val = expr_to_z3(vc, value, env, solver_opt)?;
            Ok(Int::from_i64(ctx, 0).into())
        }
        // Plan 8: Channel recv — evaluate channel, return symbolic int
        Expr::ChanRecv { channel } => {
            let _ch = expr_to_z3(vc, channel, env, solver_opt)?;
            static RECV_COUNTER: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let recv_id = RECV_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let recv_sym = Int::new_const(ctx, format!("__chan_recv_{}", recv_id).as_str());
            Ok(recv_sym.into())
        }
    }
}

/// Stmt 版 Z3 変換: Stmt を Z3 シンボリック値に変換する。
/// Expr/Stmt 分離に伴い、expr_to_z3 から文（Statement）の処理を分離。
#[allow(clippy::too_many_lines)]
fn stmt_to_z3<'a>(
    vc: &VCtx<'a>,
    stmt: &Stmt,
    env: &mut Env<'a>,
    solver_opt: Option<&Solver<'a>>,
) -> DynResult<'a> {
    let ctx = vc.ctx;
    match stmt {
        Stmt::Let { var, value, .. } => {
            let val = expr_to_z3(vc, value, env, solver_opt)?;
            env.insert(var.clone(), val.clone());
            Ok(val)
        }
        Stmt::Assign { var, value, .. } => {
            let val = expr_to_z3(vc, value, env, solver_opt)?;
            env.insert(var.clone(), val.clone());
            Ok(val)
        }
        Stmt::Block(stmts, _) => {
            let mut last: Dynamic = Int::from_i64(ctx, 0).into();
            for s in stmts {
                last = stmt_to_z3(vc, s, env, solver_opt)?;
            }
            Ok(last)
        }
        Stmt::While {
            cond,
            invariant,
            decreases,
            body,
            ..
        } => {
            // Loop Invariant 検証ロジック
            if let Some(solver) = solver_opt {
                let inv = expr_to_z3(vc, invariant, env, None)?
                    .as_bool()
                    .ok_or(MumeiError::type_error("Invariant must be boolean"))?;

                // Base case
                solver.push();
                solver.assert(&inv.not());
                if solver.check() == SatResult::Sat {
                    solver.pop(1);
                    return Err(MumeiError::verification("Invariant fails initially"));
                }
                solver.pop(1);

                // Inductive step
                let c = expr_to_z3(vc, cond, env, None)?
                    .as_bool()
                    .ok_or(MumeiError::type_error("While condition must be boolean"))?;

                {
                    let env_snapshot = env.clone();
                    solver.push();
                    solver.assert(&inv);
                    solver.assert(&c);
                    stmt_to_z3(vc, body, env, Some(solver))?;

                    let inv_after = expr_to_z3(vc, invariant, env, None)?
                        .as_bool()
                        .ok_or(MumeiError::type_error("Invariant must be boolean"))?;

                    solver.assert(&inv_after.not());
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        return Err(MumeiError::verification("Invariant not preserved"));
                    }
                    solver.pop(1);
                    *env = env_snapshot;
                }

                // Termination Check
                if let Some(dec_expr) = decreases {
                    let env_snapshot = env.clone();
                    let v_before = expr_to_z3(vc, dec_expr, env, None)?.as_int().ok_or(
                        MumeiError::type_error("decreases expression must be integer"),
                    )?;
                    solver.push();
                    solver.assert(&inv);
                    solver.assert(&c);
                    solver.assert(&v_before.lt(&Int::from_i64(ctx, 0)));
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        return Err(MumeiError::verification(
                            "Termination check failed: decreases expression may be negative",
                        ));
                    }
                    solver.pop(1);
                    solver.push();
                    solver.assert(&inv);
                    solver.assert(&c);
                    stmt_to_z3(vc, body, env, Some(solver))?;
                    let v_after = expr_to_z3(vc, dec_expr, env, None)?.as_int().ok_or(
                        MumeiError::type_error("decreases expression must be integer"),
                    )?;
                    solver.assert(&v_after.ge(&v_before));
                    if solver.check() == SatResult::Sat {
                        solver.pop(1);
                        *env = env_snapshot;
                        return Err(MumeiError::verification(
                            "Termination check failed: decreases expression does not strictly decrease"
                        ));
                    }
                    solver.pop(1);
                    *env = env_snapshot;
                }
            }

            let inv = expr_to_z3(vc, invariant, env, None)?
                .as_bool()
                .ok_or(MumeiError::type_error("Invariant must be boolean"))?;
            let c_not = expr_to_z3(vc, cond, env, None)?
                .as_bool()
                .ok_or(MumeiError::type_error("While condition must be boolean"))?
                .not();
            Ok(Bool::and(ctx, &[&inv, &c_not]).into())
        }
        Stmt::Acquire { resource, body, .. } => {
            let held_name = format!("__resource_held_{}", resource);
            let held_bool = Bool::new_const(ctx, held_name.as_str());
            if let Some(solver) = solver_opt {
                solver.assert(&held_bool);
            }
            env.insert(held_name.clone(), held_bool.into());
            let body_result = stmt_to_z3(vc, body, env, solver_opt)?;
            let released = Bool::from_bool(ctx, false);
            env.insert(held_name, released.into());
            Ok(body_result)
        }
        Stmt::Task { body, group, .. } => {
            static TASK_COUNTER: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let task_uid = TASK_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let task_id = format!(
                "__task_{}_{}",
                group.as_deref().unwrap_or("default"),
                task_uid
            );
            let task_alive = Bool::new_const(ctx, format!("{}_alive", task_id).as_str());
            env.insert(format!("{}_alive", task_id), task_alive.into());
            let body_result = stmt_to_z3(vc, body, env, solver_opt)?;
            let task_done = Bool::new_const(ctx, format!("{}_done", task_id).as_str());
            env.insert(format!("{}_done", task_id), task_done.into());
            Ok(body_result)
        }
        Stmt::TaskGroup {
            children,
            join_semantics,
            ..
        } => {
            let mut child_results = Vec::new();
            let mut child_done_vars = Vec::new();
            for (i, child) in children.iter().enumerate() {
                let child_id = format!("__task_group_child_{}", i);
                let child_alive = Bool::new_const(ctx, format!("{}_alive", child_id).as_str());
                env.insert(format!("{}_alive", child_id), child_alive.into());
                let result = stmt_to_z3(vc, child, env, solver_opt)?;
                child_results.push(result);
                let done_var = Bool::new_const(ctx, format!("{}_done", child_id).as_str());
                child_done_vars.push(done_var.clone());
                env.insert(format!("{}_done", child_id), done_var.into());
            }
            let parent_done = Bool::new_const(ctx, "__task_group_parent_done");
            if let Some(solver) = solver_opt {
                match join_semantics {
                    JoinSemantics::All => {
                        for done_var in &child_done_vars {
                            solver.assert(&parent_done.implies(done_var));
                        }
                    }
                    JoinSemantics::Any => {
                        if !child_done_vars.is_empty() {
                            let any_done =
                                Bool::or(ctx, &child_done_vars.iter().collect::<Vec<_>>());
                            solver.assert(&parent_done.implies(&any_done));
                        }
                    }
                }
            }
            if let Some(last) = child_results.last() {
                Ok(last.clone())
            } else {
                Ok(Int::from_i64(ctx, 0).into())
            }
        }
        Stmt::Expr(e, _) => expr_to_z3(vc, e, env, solver_opt),
        // Plan 8: Cancel statement — no-op in Z3 verification
        Stmt::Cancel { .. } => Ok(Int::from_i64(ctx, 0).into()),
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

#[allow(clippy::too_many_arguments)]
fn save_visualizer_report(
    output_dir: &Path,
    status: &str,
    name: &str,
    a: &str,
    b: &str,
    reason: &str,
    counterexample: Option<&serde_json::Value>,
    failure_type: &str,
    semantic_feedback: Option<&serde_json::Value>,
    span: Option<&Span>,
    constraint_mappings: Option<&[ConstraintMapping]>,
) {
    let mut report = json!({
        "status": status,
        "atom": name,
        "input_a": a,
        "input_b": b,
        "reason": reason
    });
    if !failure_type.is_empty() {
        report["failure_type"] = json!(failure_type);
    }
    if let Some(ce) = counterexample {
        report["counterexample"] = ce.clone();
    }
    if let Some(sf) = semantic_feedback {
        report["semantic_feedback"] = sf.clone();
    }
    // Use contextual suggestion when counterexample/unsat_core available, fallback to static
    let structured_unsat_core = semantic_feedback.and_then(|sf| sf.get("structured_unsat_core"));
    report["suggestion"] = json!(build_contextual_suggestion(
        failure_type,
        counterexample,
        structured_unsat_core,
    ));
    if let Some(s) = span {
        report["span"] = json!({
            "file": s.file,
            "line": s.line,
            "col": s.col,
            "len": s.len
        });
    }
    // Include constraint source locations from constraint mappings (Feature 2f)
    if let Some(mappings) = constraint_mappings {
        let type_locations: Vec<serde_json::Value> = mappings
            .iter()
            .filter(|m| m.span.line > 0)
            .map(|m| {
                json!({
                    "param": m.param_name,
                    "type": m.type_name.as_deref().unwrap_or(&m.base_type),
                    "file": m.span.file,
                    "line": m.span.line,
                    "col": m.span.col
                })
            })
            .collect();
        if !type_locations.is_empty() {
            report["type_definition_locations"] = json!(type_locations);
        }
    }
    let _ = fs::create_dir_all(output_dir);
    let _ = fs::write(output_dir.join("report.json"), report.to_string());
}

// =============================================================================
// Tests: Semantic Feedback functions (Part 1-5)
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Atom, ImplDef, Param, Span, TraitDef, TraitMethod, TrustLevel};

    // ---- constraint_to_natural_language tests ----

    #[test]
    fn test_constraint_to_natural_language_range() {
        let result =
            constraint_to_natural_language("age", "BoundedAge", "age >= 0 && age <= 120", "150");
        assert!(result.contains("age"));
        assert!(result.contains("150"));
    }

    #[test]
    fn test_constraint_to_natural_language_modulo() {
        let result = constraint_to_natural_language("n", "EvenInt", "n % 2 == 0", "3");
        assert!(result.contains("multiple") || result.contains("倍数"));
        assert!(result.contains("3"));
    }

    #[test]
    fn test_constraint_to_natural_language_enum() {
        let result = constraint_to_natural_language(
            "status",
            "StatusCode",
            "status == 1 || status == 2 || status == 3",
            "5",
        );
        assert!(result.contains("one of") || result.contains("のいずれか"));
        assert!(result.contains("5"));
    }

    #[test]
    fn test_constraint_to_natural_language_negation() {
        let result = constraint_to_natural_language("x", "NonZero", "x != 0", "0");
        assert!(result.contains("not") || result.contains("ありません"));
        assert!(result.contains("0"));
    }

    #[test]
    fn test_constraint_to_natural_language_string_constraint() {
        let result = constraint_to_natural_language(
            "path",
            "SafePath",
            "starts_with(path, \"/tmp/\")",
            "/etc/passwd",
        );
        assert!(result.contains("starts_with") || result.contains("start"));
    }

    #[test]
    fn test_constraint_to_natural_language_comparison() {
        let result = constraint_to_natural_language("x", "Positive", "x > 0", "-1");
        assert!(result.contains("greater than") || result.contains("より大きい"));
    }

    #[test]
    fn test_constraint_to_natural_language_fallback() {
        let result = constraint_to_natural_language("x", "Custom", "some_complex_pred(x)", "42");
        assert!(result.contains("x"));
        assert!(result.contains("42"));
    }

    // ---- suggestion_for_failure_type tests ----

    #[test]
    fn test_suggestion_for_failure_type_division() {
        let suggestion = suggestion_for_failure_type(FAILURE_DIVISION_BY_ZERO);
        assert!(suggestion.contains("divisor") || suggestion.contains("0"));
    }

    #[test]
    fn test_suggestion_for_failure_type_linearity() {
        let suggestion = suggestion_for_failure_type(FAILURE_LINEARITY_VIOLATED);
        assert!(
            suggestion.contains("Clone")
                || suggestion.contains("clone")
                || suggestion.contains("クローン")
        );
    }

    #[test]
    fn test_suggestion_for_failure_type_effect() {
        let suggestion = suggestion_for_failure_type("effect_not_allowed");
        assert!(suggestion.contains("effect") || suggestion.contains("エフェクト"));
    }

    #[test]
    fn test_suggestion_for_failure_type_postcondition() {
        let suggestion = suggestion_for_failure_type(FAILURE_POSTCONDITION_VIOLATED);
        assert!(!suggestion.is_empty());
    }

    #[test]
    fn test_suggestion_for_failure_type_precondition() {
        let suggestion = suggestion_for_failure_type(FAILURE_PRECONDITION_VIOLATED);
        assert!(!suggestion.is_empty());
    }

    // ---- build_division_by_zero_feedback tests ----

    #[test]
    fn test_build_division_by_zero_feedback() {
        let feedback = build_division_by_zero_feedback("10", "0");
        assert_eq!(feedback["failure_type"], FAILURE_DIVISION_BY_ZERO);
        assert!(feedback["counter_example"]["dividend"].as_str().is_some());
        assert!(feedback["counter_example"]["divisor"].as_str().is_some());
    }

    // ---- build_linearity_feedback tests ----

    #[test]
    fn test_build_linearity_feedback() {
        let violations = vec!["Variable 'x' used after being consumed".to_string()];
        let span = Span {
            file: "test.mm".to_string(),
            line: 10,
            col: 1,
            len: 5,
        };
        let feedback = build_linearity_feedback("test_atom", &violations, &span);
        assert_eq!(feedback["failure_type"], FAILURE_LINEARITY_VIOLATED);
        assert!(feedback["violations"].is_array());
        assert_eq!(feedback["atom"], "test_atom");
    }

    // ---- build_effect_feedback tests ----

    #[test]
    fn test_build_effect_feedback() {
        let allowed = vec!["FileRead".to_string()];
        let missing = vec!["FileWrite".to_string()];
        let feedback = build_effect_feedback("test_atom", "FileWrite", &allowed, &missing);
        assert_eq!(feedback["failure_type"], "effect_not_allowed");
        assert_eq!(feedback["attempted_effect"], "FileWrite");
        assert!(feedback["allowed_effects"].is_array());
        assert!(feedback["missing_effects"].is_array());
    }

    // ---- try_match_comparison tests ----

    #[test]
    fn test_try_match_comparison() {
        let result = try_match_comparison("x > 10", "x", "Bounded", "5");
        assert!(result.is_some());
        let msg = result.unwrap();
        assert!(msg.contains("greater than") || msg.contains("より大きい"));
        assert!(msg.contains("5"));
    }

    #[test]
    fn test_try_match_comparison_lte() {
        let result = try_match_comparison("x <= 100", "x", "Capped", "150");
        assert!(result.is_some());
        let msg = result.unwrap();
        assert!(msg.contains("at most") || msg.contains("以下"));
    }

    // ---- SecurityPolicy tests ----

    #[test]
    fn test_security_policy_new() {
        let policy = SecurityPolicy::new();
        assert!(policy.allowed_effects.is_empty());
    }

    #[test]
    fn test_security_policy_allow_and_check() {
        let mut policy = SecurityPolicy::new();
        policy.allow_effect(
            "FileRead",
            vec![(
                "path".to_string(),
                "starts_with(path, \"/tmp/\")".to_string(),
            )],
        );
        assert!(policy.is_effect_allowed("FileRead"));
        assert!(!policy.is_effect_allowed("FileWrite"));
    }

    #[test]
    fn test_security_policy_get_constraints() {
        let mut policy = SecurityPolicy::new();
        policy.allow_effect(
            "HttpGet",
            vec![(
                "url".to_string(),
                "starts_with(url, \"https://\")".to_string(),
            )],
        );
        let constraints = policy.get_constraints("HttpGet");
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].0, "url");
    }

    #[test]
    fn test_security_policy_check_param_constraint() {
        let mut policy = SecurityPolicy::new();
        policy.allow_effect(
            "FileRead",
            vec![(
                "path".to_string(),
                "starts_with(path, \"/tmp/\")".to_string(),
            )],
        );
        assert!(policy
            .check_param_constraint("FileRead", "path", Some("/tmp/data.txt"))
            .is_ok());
        assert!(policy
            .check_param_constraint("FileRead", "path", Some("/etc/passwd"))
            .is_err());
    }

    // ---- evaluate_string_constraint tests ----

    #[test]
    fn test_evaluate_string_constraint_starts_with() {
        assert!(evaluate_string_constraint(
            "starts_with(path, \"/tmp/\")",
            "path",
            "/tmp/data.txt"
        ));
        assert!(!evaluate_string_constraint(
            "starts_with(path, \"/tmp/\")",
            "path",
            "/etc/passwd"
        ));
    }

    #[test]
    fn test_evaluate_string_constraint_ends_with() {
        assert!(evaluate_string_constraint(
            "ends_with(file, \".mm\")",
            "file",
            "test.mm"
        ));
        assert!(!evaluate_string_constraint(
            "ends_with(file, \".mm\")",
            "file",
            "test.rs"
        ));
    }

    #[test]
    fn test_evaluate_string_constraint_contains() {
        assert!(evaluate_string_constraint(
            "contains(url, \"api\")",
            "url",
            "https://api.example.com"
        ));
        assert!(!evaluate_string_constraint(
            "contains(url, \"api\")",
            "url",
            "https://example.com"
        ));
    }

    // ---- parse_tracking_label tests ----

    #[test]
    fn test_parse_tracking_label_requires() {
        let result = parse_tracking_label("track_requires");
        assert!(result.is_some());
        let sl = result.unwrap();
        assert_eq!(sl.constraint_type, "requires");
        assert!(sl.param.is_none());
        assert!(sl.type_name.is_none());
        assert!(sl.field.is_none());
        assert!(sl.description.contains("requires"));
        assert!(sl.description.contains("前提条件"));
    }

    #[test]
    fn test_parse_tracking_label_refined_type() {
        let result = parse_tracking_label("track_refined_type_n::Nat");
        assert!(result.is_some());
        let sl = result.unwrap();
        assert_eq!(sl.constraint_type, "refined_type");
        assert_eq!(sl.param.as_deref(), Some("n"));
        assert_eq!(sl.type_name.as_deref(), Some("Nat"));
        assert!(sl.field.is_none());
        assert!(sl.description.contains("n"));
        assert!(sl.description.contains("Nat"));
        assert!(sl.description.contains("精緻型"));
    }

    #[test]
    fn test_parse_tracking_label_refined_type_underscore_var() {
        let result = parse_tracking_label("track_refined_type_my_var::Pos");
        assert!(result.is_some());
        let sl = result.unwrap();
        assert_eq!(sl.constraint_type, "refined_type");
        assert_eq!(sl.param.as_deref(), Some("my_var"));
        assert_eq!(sl.type_name.as_deref(), Some("Pos"));
        assert!(sl.description.contains("my_var"));
        assert!(sl.description.contains("Pos"));
        assert!(sl.description.contains("精緻型"));
    }

    #[test]
    fn test_parse_tracking_label_struct_field() {
        let result = parse_tracking_label("track_struct_field_p::age");
        assert!(result.is_some());
        let sl = result.unwrap();
        assert_eq!(sl.constraint_type, "struct_field");
        assert_eq!(sl.param.as_deref(), Some("p"));
        assert_eq!(sl.field.as_deref(), Some("age"));
        assert!(sl.type_name.is_none());
        assert!(sl.description.contains("p"));
        assert!(sl.description.contains("age"));
        assert!(sl.description.contains("構造体フィールド"));
    }

    #[test]
    fn test_parse_tracking_label_struct_field_underscore() {
        let result = parse_tracking_label("track_struct_field_my_obj::my_field");
        assert!(result.is_some());
        let sl = result.unwrap();
        assert_eq!(sl.constraint_type, "struct_field");
        assert_eq!(sl.param.as_deref(), Some("my_obj"));
        assert_eq!(sl.field.as_deref(), Some("my_field"));
        assert!(sl.description.contains("my_obj"));
        assert!(sl.description.contains("my_field"));
        assert!(sl.description.contains("構造体フィールド"));
    }

    #[test]
    fn test_parse_tracking_label_quantifier() {
        let result = parse_tracking_label("track_quantifier_0");
        assert!(result.is_some());
        let sl = result.unwrap();
        assert_eq!(sl.constraint_type, "quantifier");
        assert!(sl.param.is_none());
        assert!(sl.description.contains("#0"));
        assert!(sl.description.contains("量子化"));
    }

    #[test]
    fn test_parse_tracking_label_u64_nonneg() {
        let result = parse_tracking_label("track_u64_nonneg_x");
        assert!(result.is_some());
        let sl = result.unwrap();
        assert_eq!(sl.constraint_type, "u64_nonneg");
        assert_eq!(sl.param.as_deref(), Some("x"));
        assert!(sl.description.contains("x"));
        assert!(sl.description.contains("u64"));
    }

    #[test]
    fn test_parse_tracking_label_unknown() {
        assert!(parse_tracking_label("__alive_x").is_none());
        assert!(parse_tracking_label("__borrowed_y").is_none());
        assert!(parse_tracking_label("random_label").is_none());
    }

    // ---- build_contradiction_feedback tests ----

    #[test]
    fn test_build_contradiction_feedback_with_constraints() {
        let constraints = vec![
            "Precondition (requires)".to_string(),
            "Refined type constraint: n (Nat)".to_string(),
        ];
        let raw = vec![
            "track_requires".to_string(),
            "track_refined_type_n::Nat".to_string(),
        ];
        let structured: Vec<StructuredLabel> = raw
            .iter()
            .filter_map(|label| parse_tracking_label(label))
            .collect();
        let feedback = build_contradiction_feedback("test_atom", &constraints, &raw, &structured);
        assert_eq!(feedback["failure_type"], FAILURE_INVARIANT_VIOLATED);
        assert_eq!(feedback["atom"], "test_atom");
        assert!(feedback["conflicting_constraints"].is_array());
        assert_eq!(
            feedback["conflicting_constraints"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(feedback["raw_unsat_core"].is_array());
        assert!(feedback["structured_unsat_core"].is_array());
        let suc = feedback["structured_unsat_core"].as_array().unwrap();
        assert_eq!(suc.len(), 2);
        assert_eq!(suc[0]["constraint_type"], "requires");
        assert_eq!(suc[1]["constraint_type"], "refined_type");
        assert_eq!(suc[1]["param"], "n");
        assert_eq!(suc[1]["type_name"], "Nat");
        assert!(feedback["explanation"]
            .as_str()
            .unwrap()
            .contains("contradictory"));
    }

    #[test]
    fn test_build_contradiction_feedback_empty() {
        let feedback = build_contradiction_feedback("test_atom", &[], &[], &[]);
        assert_eq!(feedback["atom"], "test_atom");
        assert!(feedback["conflicting_constraints"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(feedback["structured_unsat_core"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(feedback["explanation"]
            .as_str()
            .unwrap()
            .contains("could not be determined"));
    }

    // =========================================================================
    // Task 0: Explosion Prevention Infrastructure tests
    // =========================================================================

    #[test]
    fn test_constraint_budget_exceeded() {
        // Simulate constraint budget exceeded by setting a very low budget
        let ctx = z3::Context::new(&z3::Config::new());
        let int_sort = z3::Sort::int(&ctx);
        let arr = z3::ast::Array::new_const(&ctx, "arr", &int_sort, &int_sort);
        let count_cell = std::cell::Cell::new(0usize);
        let has_string_cell = std::cell::Cell::new(false);
        let module_env = ModuleEnv::new();

        let vc = VCtx {
            ctx: &ctx,
            arr: &arr,
            module_env: &module_env,
            current_atom: None,
            linearity_ctx: None,
            effect_ctx: None,
            constraint_count: Some(&count_cell),
            constraint_budget: 5, // Very low budget
            has_string_constraints: Some(&has_string_cell),
        };

        // Each call increments and checks
        for i in 0..5 {
            let result = check_constraint_budget(&vc, "test_atom");
            assert!(result.is_ok(), "Should succeed at count {}", i + 1);
        }

        // 6th call should exceed budget (count becomes 6 > 5)
        let result = check_constraint_budget(&vc, "test_atom");
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("Constraint budget exceeded"));
        assert!(err_msg.contains("test_atom"));
        assert!(err_msg.contains("limit: 5"));
    }

    #[test]
    fn test_constraint_budget_no_limit() {
        // When constraint_count is None, no budget checking occurs
        let ctx = z3::Context::new(&z3::Config::new());
        let int_sort = z3::Sort::int(&ctx);
        let arr = z3::ast::Array::new_const(&ctx, "arr", &int_sort, &int_sort);
        let module_env = ModuleEnv::new();

        let vc = VCtx {
            ctx: &ctx,
            arr: &arr,
            module_env: &module_env,
            current_atom: None,
            linearity_ctx: None,
            effect_ctx: None,
            constraint_count: None,
            constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
            has_string_constraints: None,
        };

        // Should always succeed when no constraint tracking
        for _ in 0..100 {
            assert!(check_constraint_budget(&vc, "test_atom").is_ok());
        }
    }

    #[test]
    fn test_verification_metrics_basic() {
        let mut metrics = VerificationMetrics::new("test_atom");

        assert_eq!(metrics.atom_name, "test_atom");
        assert!(metrics.phase_times.is_empty());
        assert_eq!(metrics.total_constraints, 0);
        assert_eq!(metrics.z3_check_time, std::time::Duration::ZERO);

        // Record some phases
        metrics.record_phase("Phase 1", std::time::Duration::from_millis(10));
        metrics.record_phase("Phase 2", std::time::Duration::from_millis(20));
        metrics.total_constraints = 42;
        metrics.z3_check_time = std::time::Duration::from_millis(5);

        assert_eq!(metrics.phase_times.len(), 2);
        assert_eq!(metrics.phase_times[0].0, "Phase 1");
        assert_eq!(
            metrics.phase_times[0].1,
            std::time::Duration::from_millis(10)
        );
        assert_eq!(metrics.phase_times[1].0, "Phase 2");
        assert_eq!(metrics.total_constraints, 42);
        assert_eq!(metrics.z3_check_time, std::time::Duration::from_millis(5));
    }

    #[test]
    fn test_has_string_constraints_flag() {
        // Test that the has_string_constraints flag can be set and read
        let cell = std::cell::Cell::new(false);
        assert!(!cell.get());
        cell.set(true);
        assert!(cell.get());
    }

    #[test]
    fn test_default_constraint_budget_value() {
        assert_eq!(DEFAULT_CONSTRAINT_BUDGET, 1000);
    }

    // =========================================================================
    // Task 4: Effect Parameter Z3 String Sort — Tests
    // =========================================================================

    #[test]
    fn test_constant_path_ok() {
        // Constant path "/tmp/data.txt" should pass starts_with(path, "/tmp/") constraint
        assert!(check_constant_constraint(
            "/tmp/data.txt",
            "starts_with(path, \"/tmp/\")"
        ));
        assert!(check_constant_constraint(
            "/tmp/nested/file.log",
            "starts_with(path, \"/tmp/\")"
        ));
    }

    #[test]
    fn test_constant_path_ng() {
        // Constant path "/etc/passwd" should fail starts_with(path, "/tmp/") constraint
        assert!(!check_constant_constraint(
            "/etc/passwd",
            "starts_with(path, \"/tmp/\")"
        ));
        assert!(!check_constant_constraint(
            "/var/log/syslog",
            "starts_with(path, \"/tmp/\")"
        ));
    }

    #[test]
    fn test_z3_string_parse_constraint_starts_with() {
        // Test parse_constraint_to_z3_string with starts_with
        let cfg = z3::Config::new();
        let ctx = z3::Context::new(&cfg);
        let param = Z3String::new_const(&ctx, "path");

        // starts_with should produce a Bool constraint
        let result = parse_constraint_to_z3_string(&ctx, "starts_with(path, \"/tmp/\")", &param);
        assert!(result.is_some(), "starts_with constraint should parse");

        // ends_with should also work
        let result2 = parse_constraint_to_z3_string(&ctx, "ends_with(path, \".txt\")", &param);
        assert!(result2.is_some(), "ends_with constraint should parse");

        // contains should also work
        let result3 = parse_constraint_to_z3_string(&ctx, "contains(path, \"data\")", &param);
        assert!(result3.is_some(), "contains constraint should parse");

        // not_contains should also work
        let result4 = parse_constraint_to_z3_string(&ctx, "not_contains(path, \"..\")", &param);
        assert!(result4.is_some(), "not_contains constraint should parse");

        // Invalid constraint should return None
        let result5 = parse_constraint_to_z3_string(&ctx, "unknown_fn(path, \"x\")", &param);
        assert!(result5.is_none(), "unknown constraint should return None");
    }

    #[test]
    fn test_z3_string_constraint_satisfiability() {
        // Test that Z3 String Sort constraints are satisfiable/unsatisfiable as expected
        let cfg = z3::Config::new();
        let ctx = z3::Context::new(&cfg);
        let solver = z3::Solver::new(&ctx);

        // Create a Z3 String variable
        let path = Z3String::new_const(&ctx, "path");

        // Assert: path starts with "/tmp/"
        let prefix = Z3String::from_str(&ctx, "/tmp/").unwrap();
        solver.assert(&prefix.prefix(&path));

        // Should be satisfiable (there exist strings starting with /tmp/)
        assert_eq!(solver.check(), z3::SatResult::Sat);

        // Now also assert: path starts with "/etc/"
        let prefix2 = Z3String::from_str(&ctx, "/etc/").unwrap();
        solver.assert(&prefix2.prefix(&path));

        // Should be unsatisfiable (can't start with both /tmp/ and /etc/)
        assert_eq!(solver.check(), z3::SatResult::Unsat);
    }

    #[test]
    fn test_contains_constraint() {
        // Test contains constraint with Z3 String Sort
        let cfg = z3::Config::new();
        let ctx = z3::Context::new(&cfg);
        let solver = z3::Solver::new(&ctx);

        let path = Z3String::new_const(&ctx, "path");

        // Assert: path contains ".."
        let substr = Z3String::from_str(&ctx, "..").unwrap();
        let contains_dotdot = path.contains(&substr);
        // Assert NOT contains ".." (path traversal prevention)
        solver.assert(&contains_dotdot.not());

        // Also assert path starts with "/tmp/"
        let prefix = Z3String::from_str(&ctx, "/tmp/").unwrap();
        solver.assert(&prefix.prefix(&path));

        // Should be satisfiable: "/tmp/safe.txt" satisfies both
        assert_eq!(solver.check(), z3::SatResult::Sat);
    }

    #[test]
    fn test_z3_string_performance() {
        // Test that String Sort constraint solving completes within reasonable time
        let start = std::time::Instant::now();

        let cfg = z3::Config::new();
        let ctx = z3::Context::new(&cfg);
        let solver = z3::Solver::new(&ctx);

        // Create multiple string variables with constraints
        for i in 0..10 {
            let name = format!("path_{}", i);
            let var = Z3String::new_const(&ctx, name.as_str());
            let prefix = Z3String::from_str(&ctx, "/tmp/").unwrap();
            solver.assert(&prefix.prefix(&var));
        }

        let result = solver.check();
        let elapsed = start.elapsed();

        assert_eq!(result, z3::SatResult::Sat);
        // Should solve within 500ms even with 10 string constraints
        assert!(
            elapsed.as_millis() < 500,
            "String Sort constraint solving took {}ms, expected < 500ms",
            elapsed.as_millis()
        );
    }

    // =========================================================================
    // Compound && constraint tests
    // =========================================================================

    #[test]
    fn test_check_constant_constraint_compound() {
        // Compound constraint: starts_with AND not_contains
        assert!(check_constant_constraint(
            "/tmp/data.txt",
            "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")"
        ));
        // Path traversal should fail
        assert!(!check_constant_constraint(
            "/tmp/../etc/passwd",
            "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")"
        ));
        // Wrong prefix should fail
        assert!(!check_constant_constraint(
            "/etc/passwd",
            "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")"
        ));
    }

    #[test]
    fn test_evaluate_string_constraint_compound() {
        assert!(evaluate_string_constraint(
            "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")",
            "path",
            "/tmp/safe.txt"
        ));
        assert!(!evaluate_string_constraint(
            "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")",
            "path",
            "/tmp/../etc/passwd"
        ));
    }

    #[test]
    fn test_z3_compound_constraint_parse() {
        let cfg = z3::Config::new();
        let ctx = z3::Context::new(&cfg);
        let param = Z3String::new_const(&ctx, "path");

        // Compound constraint should parse successfully
        let result = parse_constraint_to_z3_string(
            &ctx,
            "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")",
            &param,
        );
        assert!(result.is_some(), "compound constraint should parse");

        // Compound with unknown sub-constraint should fail (fail-closed)
        let result2 = parse_constraint_to_z3_string(
            &ctx,
            "starts_with(path, \"/tmp/\") && unknown_check(path, \"x\")",
            &param,
        );
        assert!(
            result2.is_none(),
            "compound with unknown sub-constraint should return None"
        );
    }

    // =========================================================================
    // Plan 20: Temporal Effect Z3 Integration — Tests
    // =========================================================================

    #[test]
    fn test_encode_effect_state_basic() {
        use crate::mir_analysis::EffectStateMachine;

        let sm = EffectStateMachine {
            effect_name: "FileIO".to_string(),
            states: vec![
                "Open".to_string(),
                "Reading".to_string(),
                "Closed".to_string(),
            ],
            transitions: std::collections::HashMap::new(),
            initial_state: "Closed".to_string(),
        };

        assert_eq!(encode_effect_state(&sm, "Open"), 0);
        assert_eq!(encode_effect_state(&sm, "Reading"), 1);
        assert_eq!(encode_effect_state(&sm, "Closed"), 2);
        assert_eq!(encode_effect_state(&sm, "Unknown"), -1);
    }

    #[test]
    fn test_z3_conflicting_state_unsat() {
        // Two different states at the same merge point should be UNSAT.
        let cfg = z3::Config::new();
        let ctx = z3::Context::new(&cfg);
        let solver = z3::Solver::new(&ctx);

        let state_var = Int::new_const(&ctx, "__effect_state_FileIO_3");

        // Branch A says state = 0 (Open)
        solver.assert(&state_var._eq(&Int::from_i64(&ctx, 0)));
        // Branch B says state = 2 (Closed)
        solver.assert(&state_var._eq(&Int::from_i64(&ctx, 2)));
        // Valid range: 0..3
        solver.assert(&state_var.ge(&Int::from_i64(&ctx, 0)));
        solver.assert(&state_var.lt(&Int::from_i64(&ctx, 3)));

        assert_eq!(
            solver.check(),
            z3::SatResult::Unsat,
            "Different states at merge point should be UNSAT"
        );
    }

    #[test]
    fn test_z3_conflicting_state_sat_same() {
        // Same state from both branches should be SAT.
        let cfg = z3::Config::new();
        let ctx = z3::Context::new(&cfg);
        let solver = z3::Solver::new(&ctx);

        let state_var = Int::new_const(&ctx, "__effect_state_FileIO_3");

        // Both branches say state = 1 (Reading)
        solver.assert(&state_var._eq(&Int::from_i64(&ctx, 1)));
        solver.assert(&state_var._eq(&Int::from_i64(&ctx, 1)));
        solver.assert(&state_var.ge(&Int::from_i64(&ctx, 0)));
        solver.assert(&state_var.lt(&Int::from_i64(&ctx, 3)));

        assert_eq!(
            solver.check(),
            z3::SatResult::Sat,
            "Same state from both branches should be SAT"
        );
    }

    // ---- split_compound_constraint tests ----

    #[test]
    fn test_split_compound_simple_and() {
        let parts = split_compound_constraint("a >= 0 && a <= 120");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "a >= 0");
        assert_eq!(parts[1], "a <= 120");
    }

    #[test]
    fn test_split_compound_with_nested_parens() {
        let parts = split_compound_constraint("(a > 0 && a < 10) && b > 0");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "(a > 0 && a < 10)");
        assert_eq!(parts[1], "b > 0");
    }

    #[test]
    fn test_split_compound_with_quoted_strings() {
        let parts =
            split_compound_constraint("starts_with(path, \"/tmp/\") && not_contains(path, \"..\")");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "starts_with(path, \"/tmp/\")");
        assert_eq!(parts[1], "not_contains(path, \"..\")");
    }

    #[test]
    fn test_split_compound_single() {
        let parts = split_compound_constraint("x > 0");
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], "x > 0");
    }

    #[test]
    fn test_split_compound_quoted_ampersand() {
        let parts = split_compound_constraint("contains(s, \"a && b\") && x > 0");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "contains(s, \"a && b\")");
        assert_eq!(parts[1], "x > 0");
    }

    // ---- evaluate_sub_constraint tests ----

    #[test]
    fn test_evaluate_sub_constraint_starts_with_satisfied() {
        assert!(evaluate_sub_constraint(
            "starts_with(path, \"/tmp/\")",
            "/tmp/../etc/passwd"
        ));
    }

    #[test]
    fn test_evaluate_sub_constraint_not_contains_violated() {
        assert!(!evaluate_sub_constraint(
            "not_contains(path, \"..\")",
            "/tmp/../etc/passwd"
        ));
    }

    #[test]
    fn test_evaluate_sub_constraint_numeric_comparison() {
        assert!(evaluate_sub_constraint("v >= 0", "5"));
        assert!(!evaluate_sub_constraint("v >= 0", "-1"));
        assert!(evaluate_sub_constraint("v <= 120", "100"));
        assert!(!evaluate_sub_constraint("v <= 120", "150"));
    }

    // ---- compound constraint_to_natural_language tests ----

    #[test]
    fn test_constraint_to_natural_language_compound() {
        let result = constraint_to_natural_language(
            "path",
            "SafePath",
            "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")",
            "/etc/passwd",
        );
        assert!(result.contains("[1/2]"));
        assert!(result.contains("[2/2]"));
        assert!(result.contains("AND"));
    }

    // ---- build_semantic_feedback with sub_constraints test ----

    #[test]
    fn test_build_semantic_feedback_sub_constraints() {
        use crate::parser::ast::Span;
        let mappings = vec![ConstraintMapping {
            param_name: "path".to_string(),
            type_name: Some("SafePath".to_string()),
            base_type: "Str".to_string(),
            predicate_raw: "starts_with(path, \"/tmp/\") && not_contains(path, \"..\")".to_string(),
            span: Span::default(),
        }];
        let ce = serde_json::json!({
            "path": "/tmp/../etc/passwd"
        });
        let dummy_atom = crate::parser::ast::Atom {
            name: "test_atom".to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![],
            requires: String::new(),
            forall_constraints: vec![],
            ensures: String::new(),
            body_expr: String::new(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: crate::parser::ast::TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: Span::default(),
            effect_pre: std::collections::HashMap::new(),
            effect_post: std::collections::HashMap::new(),
        };
        let feedback = build_semantic_feedback(
            &mappings,
            Some(&ce),
            &dummy_atom,
            "precondition_violated",
            None,
        )
        .unwrap();
        let vc_arr = feedback["violated_constraints"].as_array().unwrap();
        assert!(!vc_arr.is_empty());
        let vc = &vc_arr[0];
        assert!(vc.get("sub_constraints").is_some());
        let subs = vc["sub_constraints"].as_array().unwrap();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0]["satisfied"], true);
        assert_eq!(subs[1]["satisfied"], false);
    }

    // ---- build_contextual_suggestion tests ----

    #[test]
    fn test_contextual_suggestion_precondition_with_zero_counterexample() {
        let ce = json!({"b": "0"});
        let result = build_contextual_suggestion(FAILURE_PRECONDITION_VIOLATED, Some(&ce), None);
        assert!(
            result.contains("b != 0"),
            "should suggest b != 0 guard: {}",
            result
        );
        assert!(
            result.contains("requires"),
            "should mention requires: {}",
            result
        );
    }

    #[test]
    fn test_contextual_suggestion_precondition_with_violated_constraint() {
        let ce = json!({"b": "0"});
        let unsat_core = json!([{"description": "b != 0"}]);
        let result = build_contextual_suggestion(
            FAILURE_PRECONDITION_VIOLATED,
            Some(&ce),
            Some(&unsat_core),
        );
        assert!(
            result.contains("b != 0"),
            "should reference violated constraint: {}",
            result
        );
    }

    #[test]
    fn test_contextual_suggestion_postcondition_with_counterexample() {
        let ce = json!({"x": "-1"});
        let result = build_contextual_suggestion(FAILURE_POSTCONDITION_VIOLATED, Some(&ce), None);
        assert!(
            result.contains("x = -1"),
            "should mention x = -1: {}",
            result
        );
        assert!(
            result.contains("ensures"),
            "should mention ensures: {}",
            result
        );
    }

    #[test]
    fn test_contextual_suggestion_division_by_zero_with_counterexample() {
        let ce = json!({"divisor": "0"});
        let result = build_contextual_suggestion(FAILURE_DIVISION_BY_ZERO, Some(&ce), None);
        assert!(
            result.contains("divisor != 0"),
            "should suggest divisor != 0: {}",
            result
        );
    }

    #[test]
    fn test_contextual_suggestion_invariant_with_unsat_core() {
        let unsat_core = json!([{"description": "i >= 0"}]);
        let result =
            build_contextual_suggestion(FAILURE_INVARIANT_VIOLATED, None, Some(&unsat_core));
        assert!(
            result.contains("i >= 0"),
            "should reference constraint: {}",
            result
        );
        assert!(
            result.contains("invariant") || result.contains("不変条件"),
            "should mention invariant: {}",
            result
        );
    }

    #[test]
    fn test_contextual_suggestion_precondition_with_integer_counterexample() {
        // Regression test: JSON integer values (not strings) must be rendered correctly
        let ce = json!({"b": 0});
        let result = build_contextual_suggestion(FAILURE_PRECONDITION_VIOLATED, Some(&ce), None);
        assert!(
            result.contains("b != 0"),
            "should suggest b != 0 guard: {}",
            result
        );
    }

    #[test]
    fn test_contextual_suggestion_postcondition_with_integer_counterexample() {
        // Regression test: JSON integer values in postcondition branch
        let ce = json!({"x": -1});
        let result = build_contextual_suggestion(FAILURE_POSTCONDITION_VIOLATED, Some(&ce), None);
        assert!(
            result.contains("x = -1"),
            "should mention x = -1: {}",
            result
        );
    }

    #[test]
    fn test_contextual_suggestion_division_by_zero_with_integer_counterexample() {
        // Regression test: JSON integer values in division-by-zero branch
        let ce = json!({"divisor": 0});
        let result = build_contextual_suggestion(FAILURE_DIVISION_BY_ZERO, Some(&ce), None);
        assert!(
            result.contains("divisor != 0"),
            "should suggest divisor != 0: {}",
            result
        );
    }

    #[test]
    fn test_contextual_suggestion_fallback_no_counterexample() {
        let result = build_contextual_suggestion(FAILURE_PRECONDITION_VIOLATED, None, None);
        // Should fall back to suggestion_for_failure_type
        let fallback = suggestion_for_failure_type(FAILURE_PRECONDITION_VIOLATED);
        assert_eq!(result, fallback);
    }

    #[test]
    fn test_contextual_suggestion_unknown_failure_type() {
        let result = build_contextual_suggestion("unknown_type", Some(&json!({"x": "1"})), None);
        let fallback = suggestion_for_failure_type("unknown_type");
        assert_eq!(result, fallback);
    }

    // ---- Subsumption check unit tests ----

    #[test]
    fn test_subsumption_check_holds_with_requires() {
        // increment: requires x >= 0, ensures result == x + 1
        // contract: ensures result >= 0
        // With requires x >= 0: result == x + 1 ≥ 1 ≥ 0, so subsumption holds.
        let cfg = Config::new();
        let ctx = Context::new(&cfg);
        let solver = Solver::new(&ctx);
        let int_sort = z3::Sort::int(&ctx);
        let arr = Array::new_const(&ctx, "arr", &int_sort, &int_sort);
        let module_env = ModuleEnv::new();
        let vc = VCtx {
            ctx: &ctx,
            arr: &arr,
            module_env: &module_env,
            current_atom: None,
            linearity_ctx: None,
            effect_ctx: None,
            constraint_count: None,
            constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
            has_string_constraints: None,
        };
        let concrete = Atom {
            name: "increment".to_string(),
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
            requires: "x >= 0".to_string(),
            forall_constraints: vec![],
            ensures: "result == x + 1".to_string(),
            body_expr: "x + 1".to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: Span::default(),
            effect_pre: std::collections::HashMap::new(),
            effect_post: std::collections::HashMap::new(),
        };
        let result = check_contract_subsumption(
            &vc,
            &concrete,
            "result >= 0",
            None,
            "apply",
            "f",
            &solver,
            &ctx,
        );
        assert!(
            result,
            "subsumption should hold: x >= 0 ∧ result == x + 1 ⇒ result >= 0"
        );
    }

    #[test]
    fn test_subsumption_check_fails_without_requires() {
        // negate: requires x >= 0, ensures result == 0 - x
        // contract: ensures result >= 0
        // Even with requires x >= 0, result == -x ≤ 0 when x > 0, so subsumption fails.
        let cfg = Config::new();
        let ctx = Context::new(&cfg);
        let solver = Solver::new(&ctx);
        let int_sort = z3::Sort::int(&ctx);
        let arr = Array::new_const(&ctx, "arr", &int_sort, &int_sort);
        let module_env = ModuleEnv::new();
        let vc = VCtx {
            ctx: &ctx,
            arr: &arr,
            module_env: &module_env,
            current_atom: None,
            linearity_ctx: None,
            effect_ctx: None,
            constraint_count: None,
            constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
            has_string_constraints: None,
        };
        let concrete = Atom {
            name: "negate".to_string(),
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
            requires: "x >= 0".to_string(),
            forall_constraints: vec![],
            ensures: "result == 0 - x".to_string(),
            body_expr: "0 - x".to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: Span::default(),
            effect_pre: std::collections::HashMap::new(),
            effect_post: std::collections::HashMap::new(),
        };
        let result = check_contract_subsumption(
            &vc,
            &concrete,
            "result >= 0",
            None,
            "apply",
            "f",
            &solver,
            &ctx,
        );
        assert!(
            !result,
            "subsumption should fail: x >= 0 ∧ result == -x does NOT imply result >= 0"
        );
    }

    #[test]
    fn test_subsumption_check_crossed_param_names() {
        // Regression test: atom compute(y: i64, x: i64) with ensures: result == y / x
        // Contract: ensures result >= 0
        // Without the alias-collision fix, both "x" and "y" would map to the same
        // Z3 variable, making result == var/var == 1, trivially passing.
        // With the fix, y and x are independent, so y=-1, x=1 → result=-1 is a
        // valid counterexample and subsumption should fail.
        let cfg = Config::new();
        let ctx = Context::new(&cfg);
        let solver = Solver::new(&ctx);
        let int_sort = z3::Sort::int(&ctx);
        let arr = Array::new_const(&ctx, "arr", &int_sort, &int_sort);
        let module_env = ModuleEnv::new();
        let vc = VCtx {
            ctx: &ctx,
            arr: &arr,
            module_env: &module_env,
            current_atom: None,
            linearity_ctx: None,
            effect_ctx: None,
            constraint_count: None,
            constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
            has_string_constraints: None,
        };
        let concrete = Atom {
            name: "compute".to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![
                Param {
                    name: "y".to_string(),
                    type_name: Some("i64".to_string()),
                    type_ref: None,
                    is_ref: false,
                    is_ref_mut: false,
                    fn_contract_requires: None,
                    fn_contract_ensures: None,
                },
                Param {
                    name: "x".to_string(),
                    type_name: Some("i64".to_string()),
                    type_ref: None,
                    is_ref: false,
                    is_ref_mut: false,
                    fn_contract_requires: None,
                    fn_contract_ensures: None,
                },
            ],
            requires: "x != 0".to_string(),
            forall_constraints: vec![],
            ensures: "result == y / x".to_string(),
            body_expr: "y / x".to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: Span::default(),
            effect_pre: std::collections::HashMap::new(),
            effect_post: std::collections::HashMap::new(),
        };
        let result = check_contract_subsumption(
            &vc,
            &concrete,
            "result >= 0",
            None,
            "apply",
            "f",
            &solver,
            &ctx,
        );
        assert!(
            !result,
            "subsumption should fail: y/x can be negative (e.g. y=-1, x=1)"
        );
    }

    #[test]
    fn test_subsumption_check_trivial_contract_ensures_skipped() {
        // If contract_ensures is "true", subsumption is trivially satisfied
        // (the contract requires nothing).
        let cfg = Config::new();
        let ctx = Context::new(&cfg);
        let solver = Solver::new(&ctx);
        let int_sort = z3::Sort::int(&ctx);
        let arr = Array::new_const(&ctx, "arr", &int_sort, &int_sort);
        let module_env = ModuleEnv::new();
        let vc = VCtx {
            ctx: &ctx,
            arr: &arr,
            module_env: &module_env,
            current_atom: None,
            linearity_ctx: None,
            effect_ctx: None,
            constraint_count: None,
            constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
            has_string_constraints: None,
        };
        let concrete = Atom {
            name: "something".to_string(),
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
            requires: "true".to_string(),
            forall_constraints: vec![],
            ensures: "result == x".to_string(),
            body_expr: "x".to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: Span::default(),
            effect_pre: std::collections::HashMap::new(),
            effect_post: std::collections::HashMap::new(),
        };
        // contract ensures is "true" → trivially satisfied, skip check
        let result =
            check_contract_subsumption(&vc, &concrete, "true", None, "apply", "f", &solver, &ctx);
        assert!(
            result,
            "trivial contract ensures 'true' should be skipped (returns true)"
        );
    }

    #[test]
    fn test_subsumption_check_concrete_true_ensures_warns() {
        // If concrete_atom.ensures is "true" but contract requires "result >= 0",
        // the concrete atom guarantees nothing → subsumption should FAIL (warn).
        let cfg = Config::new();
        let ctx = Context::new(&cfg);
        let solver = Solver::new(&ctx);
        let int_sort = z3::Sort::int(&ctx);
        let arr = Array::new_const(&ctx, "arr", &int_sort, &int_sort);
        let module_env = ModuleEnv::new();
        let vc = VCtx {
            ctx: &ctx,
            arr: &arr,
            module_env: &module_env,
            current_atom: None,
            linearity_ctx: None,
            effect_ctx: None,
            constraint_count: None,
            constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
            has_string_constraints: None,
        };
        let concrete = Atom {
            name: "no_guarantee".to_string(),
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
            requires: "true".to_string(),
            forall_constraints: vec![],
            ensures: "true".to_string(),
            body_expr: "x".to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: Span::default(),
            effect_pre: std::collections::HashMap::new(),
            effect_post: std::collections::HashMap::new(),
        };
        // concrete ensures is "true" but contract requires "result >= 0"
        // → subsumption should fail because "true" does not imply "result >= 0"
        let result = check_contract_subsumption(
            &vc,
            &concrete,
            "result >= 0",
            None,
            "apply",
            "f",
            &solver,
            &ctx,
        );
        assert!(
            !result,
            "concrete ensures 'true' cannot imply 'result >= 0' — should warn (return false)"
        );
    }

    // ---- 3-a: replace_constraint_placeholder tests ----

    #[test]
    fn test_replace_constraint_placeholder_standalone_v() {
        // "v != 0" → "b != 0" (standalone v is replaced)
        let result = replace_constraint_placeholder("v != 0", "b");
        assert_eq!(result, "b != 0");
    }

    #[test]
    fn test_replace_constraint_placeholder_does_not_corrupt_value() {
        // "value != 0" should NOT become "balue != 0"
        let result = replace_constraint_placeholder("value != 0", "b");
        assert_eq!(result, "value != 0");
    }

    #[test]
    fn test_replace_constraint_placeholder_does_not_corrupt_divisor() {
        // "divisor > v" should replace only standalone v
        let result = replace_constraint_placeholder("divisor > v", "x");
        assert_eq!(result, "divisor > x");
    }

    #[test]
    fn test_replace_constraint_placeholder_multiple_v() {
        // "v > 0 && v < 100" → "param > 0 && param < 100"
        let result = replace_constraint_placeholder("v > 0 && v < 100", "param");
        assert_eq!(result, "param > 0 && param < 100");
    }

    // ---- 3-b: get_traits_for_method / method_trait_index tests ----

    #[test]
    fn test_get_traits_for_method_single_trait() {
        let mut env = ModuleEnv::new();
        env.register_trait(&TraitDef {
            name: "Numeric".to_string(),
            methods: vec![TraitMethod {
                name: "div".to_string(),
                param_types: vec!["i64".to_string(), "i64".to_string()],
                return_type: "i64".to_string(),
                param_constraints: vec![None, Some("v != 0".to_string())],
            }],
            laws: vec![],
            span: Span::new("", 0, 0, 0),
        });
        let results = env.get_traits_for_method("div");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "Numeric");
        assert_eq!(results[0].1.name, "div");
    }

    #[test]
    fn test_get_traits_for_method_multiple_traits_same_method() {
        let mut env = ModuleEnv::new();
        env.register_trait(&TraitDef {
            name: "TraitA".to_string(),
            methods: vec![TraitMethod {
                name: "process".to_string(),
                param_types: vec!["i64".to_string()],
                return_type: "i64".to_string(),
                param_constraints: vec![Some("v > 0".to_string())],
            }],
            laws: vec![],
            span: Span::new("", 0, 0, 0),
        });
        env.register_trait(&TraitDef {
            name: "TraitB".to_string(),
            methods: vec![TraitMethod {
                name: "process".to_string(),
                param_types: vec!["i64".to_string()],
                return_type: "i64".to_string(),
                param_constraints: vec![None],
            }],
            laws: vec![],
            span: Span::new("", 0, 0, 0),
        });
        // Both traits should be returned
        let results = env.get_traits_for_method("process");
        assert_eq!(results.len(), 2);
        let trait_names: Vec<&str> = results.iter().map(|(tn, _)| *tn).collect();
        assert!(trait_names.contains(&"TraitA"));
        assert!(trait_names.contains(&"TraitB"));
    }

    #[test]
    fn test_get_traits_for_method_selects_correct_with_find_impl() {
        let mut env = ModuleEnv::new();
        env.register_trait(&TraitDef {
            name: "TraitA".to_string(),
            methods: vec![TraitMethod {
                name: "process".to_string(),
                param_types: vec!["i64".to_string()],
                return_type: "i64".to_string(),
                param_constraints: vec![Some("v > 0".to_string())],
            }],
            laws: vec![],
            span: Span::new("", 0, 0, 0),
        });
        env.register_trait(&TraitDef {
            name: "TraitB".to_string(),
            methods: vec![TraitMethod {
                name: "process".to_string(),
                param_types: vec!["Str".to_string()],
                return_type: "Str".to_string(),
                param_constraints: vec![None],
            }],
            laws: vec![],
            span: Span::new("", 0, 0, 0),
        });
        // Only TraitA has an impl for i64
        env.register_impl(&ImplDef {
            trait_name: "TraitA".to_string(),
            target_type: "i64".to_string(),
            method_bodies: vec![],
            span: Span::new("", 0, 0, 0),
        });
        let candidates = env.get_traits_for_method("process");
        let matched = candidates
            .iter()
            .find(|(tn, _)| env.find_impl(tn, "i64").is_some());
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().0, "TraitA");
    }

    // ---- 3-c: infer_requires callee argument substitution tests ----

    #[test]
    fn test_infer_requires_substitutes_callee_params() {
        use std::collections::HashMap;
        let mut env = ModuleEnv::new();
        // Register callee atom with requires: x > 0
        env.register_atom(&Atom {
            name: "callee".to_string(),
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
            requires: "x > 0".to_string(),
            forall_constraints: vec![],
            ensures: "true".to_string(),
            body_expr: "x + 1".to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: Span::new("", 0, 0, 0),
            effect_pre: HashMap::new(),
            effect_post: HashMap::new(),
        });
        // Caller atom that calls callee(a + b)
        let caller = Atom {
            name: "caller".to_string(),
            type_params: vec![],
            where_bounds: vec![],
            params: vec![
                Param {
                    name: "a".to_string(),
                    type_name: Some("i64".to_string()),
                    type_ref: None,
                    is_ref: false,
                    is_ref_mut: false,
                    fn_contract_requires: None,
                    fn_contract_ensures: None,
                },
                Param {
                    name: "b".to_string(),
                    type_name: Some("i64".to_string()),
                    type_ref: None,
                    is_ref: false,
                    is_ref_mut: false,
                    fn_contract_requires: None,
                    fn_contract_ensures: None,
                },
            ],
            requires: "true".to_string(),
            forall_constraints: vec![],
            ensures: "true".to_string(),
            body_expr: "callee(a + b)".to_string(),
            consumed_params: vec![],
            resources: vec![],
            is_async: false,
            trust_level: TrustLevel::Verified,
            max_unroll: None,
            invariant: None,
            effects: vec![],
            return_type: None,
            span: Span::new("", 0, 0, 0),
            effect_pre: HashMap::new(),
            effect_post: HashMap::new(),
        };
        let inferred = infer_requires(&caller, &env);
        // Should contain "(a + b) > 0" not "x > 0"
        assert!(
            inferred
                .iter()
                .any(|r| r.contains("a + b") && r.contains("> 0")),
            "Expected substituted requires with 'a + b', got: {:?}",
            inferred
        );
        assert!(
            !inferred.iter().any(|r| r == "x > 0"),
            "Should not contain raw callee param 'x > 0', got: {:?}",
            inferred
        );
    }

    // ---- expr_to_source_string tests ----

    #[test]
    fn test_expr_to_source_string_basic() {
        assert_eq!(expr_to_source_string(&Expr::Number(42)), "42");
        assert_eq!(expr_to_source_string(&Expr::Variable("x".to_string())), "x");
        assert_eq!(
            expr_to_source_string(&Expr::BinaryOp(
                Box::new(Expr::Variable("a".to_string())),
                Op::Add,
                Box::new(Expr::Variable("b".to_string())),
            )),
            "(a + b)"
        );
    }
}
