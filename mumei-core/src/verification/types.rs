use super::module_env::ModuleEnv;
use super::*;
use crate::lowering::{lower, LoweredType};

/// Word-boundary-aware replacement of the placeholder `v` in constraint strings.
/// Uses regex `\bv\b` to avoid corrupting identifiers like "value" or "divisor".
pub(crate) fn replace_constraint_placeholder(constraint: &str, replacement: &str) -> String {
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
    /// Z3 から抽出した反例（counter-example）。
    /// エディタや LSP がインライン装飾で表示するために利用する。
    pub counterexample: Option<serde_json::Value>,
}

impl ErrorDetail {
    /// メッセージのみで ErrorDetail を生成する（Span 不明時のフォールバック用）
    #[allow(dead_code)]
    pub fn from_message(msg: impl Into<String>) -> Self {
        ErrorDetail {
            message: msg.into(),
            span: Span::default(),
            suggestion: None,
            counterexample: None,
        }
    }

    /// Span 付きで ErrorDetail を生成する
    #[allow(dead_code)]
    pub fn with_span(msg: impl Into<String>, span: Span) -> Self {
        ErrorDetail {
            message: msg.into(),
            span,
            suggestion: None,
            counterexample: None,
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
        /// Z3 から抽出した反例（counter-example）。
        /// エディタや LSP がインライン装飾用に取り出して表示する。
        counterexample: Option<serde_json::Value>,
    },
    #[error("Contract Mutation: atom '{atom_name}' contract hash changed from {expected_hash} to {actual_hash}")]
    #[diagnostic(code(mumei::contract_mutation))]
    ContractMutation {
        atom_name: String,
        expected_hash: String,
        actual_hash: String,
        #[source_code]
        src: miette::NamedSource<String>,
        #[label("contract mutated here")]
        span: SourceSpan,
        #[help]
        help: Option<String>,
        original_span: Span,
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
            counterexample: None,
        }
    }
    pub fn contract_mutation(
        atom_name: impl Into<String>,
        expected_hash: impl Into<String>,
        actual_hash: impl Into<String>,
        span: Span,
    ) -> Self {
        MumeiError::ContractMutation {
            atom_name: atom_name.into(),
            expected_hash: expected_hash.into(),
            actual_hash: actual_hash.into(),
            src: miette::NamedSource::new(
                if span.file.is_empty() {
                    "<unknown>"
                } else {
                    &span.file
                },
                String::new(),
            ),
            span: SourceSpan::from((0, 0)),
            help: Some(
                "Specification changes are not allowed in contract isolation mode; modify only the implementation body."
                    .to_string(),
            ),
            original_span: span,
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
            counterexample: None,
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
            counterexample: None,
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

    /// ErrorDetail を取得する（Span 情報と counter-example を保持）
    pub fn to_detail(&self) -> ErrorDetail {
        match self {
            MumeiError::VerificationError {
                msg,
                original_span,
                counterexample,
                ..
            } => {
                let mut detail = ErrorDetail::with_span(
                    format!("Verification Error: {}", msg),
                    original_span.clone(),
                );
                detail.counterexample = counterexample.clone();
                detail
            }
            MumeiError::ContractMutation {
                atom_name,
                expected_hash,
                actual_hash,
                original_span,
                ..
            } => ErrorDetail::with_span(
                format!(
                    "Contract Mutation: atom '{}' contract hash changed from {} to {}",
                    atom_name, expected_hash, actual_hash
                ),
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

    /// Z3 反例（counter-example）をエラーに附与する。VerificationError 以外の variant では no-op。
    pub fn with_counterexample(mut self, ce: Option<serde_json::Value>) -> Self {
        if let MumeiError::VerificationError { counterexample, .. } = &mut self {
            *counterexample = ce;
        }
        self
    }

    /// ソースコードを設定してリッチ出力を有効にする
    /// エラー自身が意味のある original_span を持つ場合はそちらを優先し、
    /// そうでなければ fallback_span（atom 定義の span 等）を使用する。
    pub fn with_source(self, source: &str, fallback_span: &Span) -> Self {
        // Use the error's own original_span if it has meaningful location info,
        // otherwise fall back to the provided span (e.g., atom definition)
        let effective_span = match &self {
            MumeiError::VerificationError { original_span, .. }
            | MumeiError::ContractMutation { original_span, .. }
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
                counterexample,
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
                    counterexample,
                }
            }
            MumeiError::ContractMutation {
                atom_name,
                expected_hash,
                actual_hash,
                help,
                original_span,
                ..
            } => MumeiError::ContractMutation {
                atom_name,
                expected_hash,
                actual_hash,
                src: named_src,
                span: source_span,
                help,
                original_span,
            },
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
                counterexample,
                ..
            } => MumeiError::VerificationError {
                msg,
                src,
                span,
                help,
                original_span,
                related,
                counterexample,
            },
            MumeiError::ContractMutation {
                atom_name,
                expected_hash,
                actual_hash,
                src,
                span,
                original_span,
                ..
            } => MumeiError::ContractMutation {
                atom_name,
                expected_hash,
                actual_hash,
                src,
                span,
                help,
                original_span,
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
            MumeiError::ContractMutation { .. } => {}
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationConfig {
    pub timeout_ms: u64,
    pub global_max_unroll: usize,
    pub enable_cross_spec_verification: bool,
    pub collect_decidable_fragment_metrics: bool,
    pub enable_spurious_detection: bool,
    pub enable_vacuity_check: bool,
    pub detect_loops: bool,
    pub suggest_cegis: bool,
    /// Opt-in IEEE 754 binary64 encoding for `f64` verification
    /// (`--ieee754-f64`). Default `false` keeps the exact-rational `Real`
    /// encoding, preserving decidability/speed for existing fixtures.
    pub ieee754_f64: bool,
    pub property_based_test: Option<PropertyBasedTestConfig>,
}

impl Default for VerificationConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 10000,
            global_max_unroll: 3,
            enable_cross_spec_verification: false,
            collect_decidable_fragment_metrics: false,
            enable_spurious_detection: true,
            enable_vacuity_check: false,
            detect_loops: false,
            suggest_cegis: false,
            ieee754_f64: false,
            property_based_test: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct DecidableFragmentMetrics {
    pub total_atoms_checked: usize,
    pub atoms_with_warnings: usize,
    pub warning_counts: HashMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: String,
    pub atom: String,
    pub message: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation_reason: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ModuleVerificationReport {
    pub cross_spec: Option<CrossSpecResult>,
    pub decidable_fragment: Option<DecidableFragmentMetrics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_vector: Option<serde_json::Value>,
}

pub type VerificationReport = ModuleVerificationReport;

pub const LEAN_TRANSLATOR_VERSION: &str = "mumei-lean-translator-ir-v1";
pub const LEAN_BRIDGE_LEMMA_HASH: &str =
    "a8fd0b115fd29a6e87190bd041dbd5ab7a09ec89af6ac5b10ef152a1a0c0f643";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct TranslatorIRProvenanceSpan {
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub line: usize,
    #[serde(default)]
    pub col: usize,
    #[serde(default)]
    pub len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct TranslatorIRBinder {
    #[serde(default)]
    pub mumei_name: String,
    #[serde(default)]
    pub lean_name: String,
    #[serde(default)]
    pub mumei_type: String,
    #[serde(default)]
    pub lean_type: String,
    #[serde(default)]
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refinement: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct TranslatorIRMetadata {
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub binders: Vec<TranslatorIRBinder>,
    #[serde(default)]
    pub theorem_goal: String,
    #[serde(default)]
    pub provenance_span: TranslatorIRProvenanceSpan,
    #[serde(default)]
    pub lowering_rules: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_lemma_reason: Option<String>,
    #[serde(default)]
    pub semantic_gap_notes: Vec<String>,
    #[serde(default)]
    pub proof_trace_hints: Vec<String>,
    #[serde(default)]
    pub requires_bridge_lemmas: Vec<String>,
}

pub fn build_translator_binder_mapping(atom: &Atom) -> HashMap<String, String> {
    build_translator_ir_metadata(atom, &ModuleEnv::default())
        .binders
        .into_iter()
        .map(|binder| (binder.mumei_name, binder.lean_name))
        .collect()
}

pub fn build_translator_ir_metadata(atom: &Atom, module_env: &ModuleEnv) -> TranslatorIRMetadata {
    let body_stmt = parse_body_expr(&atom.body_expr);
    let tags = detect_logic_fragment_tags(atom, module_env);
    let sort = if atom.invariant.is_some() || stmt_has_while(&body_stmt) {
        "loop_invariant"
    } else if atom.params.iter().any(|param| {
        param
            .type_name
            .as_ref()
            .is_some_and(|type_name| module_env.types.contains_key(type_name))
    }) {
        "refinement_predicate"
    } else if tags.iter().any(|tag| tag == "inductive_data_type") {
        "inductive_obligation"
    } else {
        "contract_obligation"
    };

    let mut binders: Vec<TranslatorIRBinder> = atom
        .params
        .iter()
        .map(|param| {
            let mumei_type = param
                .type_ref
                .as_ref()
                .map(TypeRef::display_name)
                .or_else(|| param.type_name.clone())
                .unwrap_or_else(|| "i64".to_string());
            let refinement = param
                .type_name
                .as_ref()
                .and_then(|type_name| module_env.types.get(type_name))
                .map(|refined| refined.predicate_raw.clone());
            TranslatorIRBinder {
                mumei_name: param.name.clone(),
                lean_name: lean_binder_name(&param.name),
                lean_type: mumei_type_to_lean_type(&mumei_type),
                mumei_type,
                role: "param".to_string(),
                refinement,
            }
        })
        .collect();

    let result_type = atom.return_type.as_deref().unwrap_or("i64").to_string();
    binders.push(TranslatorIRBinder {
        mumei_name: "result".to_string(),
        lean_name: "result".to_string(),
        lean_type: mumei_type_to_lean_type(&result_type),
        mumei_type: result_type,
        role: "result".to_string(),
        refinement: None,
    });

    let mut lowering_rules = vec![
        "type_system_mapping".to_string(),
        "contract_lowering".to_string(),
    ];
    if sort == "loop_invariant" {
        lowering_rules.push("loop_invariant_recursion".to_string());
    }
    if !atom.effects.is_empty() {
        lowering_rules.push("effect_state_bridge".to_string());
    }
    if tags.iter().any(|tag| tag.contains("array")) {
        lowering_rules.push("array_bounds_bridge".to_string());
    }
    if tags.iter().any(|tag| tag.contains("string")) {
        lowering_rules.push("string_regex_bridge".to_string());
    }
    if tags.iter().any(|tag| tag.contains("nonlinear")) {
        lowering_rules.push("integer_overflow_bridge".to_string());
    }
    if tags
        .iter()
        .any(|tag| tag.contains("quantifier") || tag.contains("refinement"))
    {
        lowering_rules.push("refinement_predicate_lowering".to_string());
    }
    if tags.iter().any(|tag| tag == "finite_field") {
        lowering_rules.push("finite_field_lowering".to_string());
        lowering_rules.push("mathlib4_bridge".to_string());
    }

    lowering_rules.sort();
    lowering_rules.dedup();
    let semantic_gap_notes = semantic_gap_notes_for_rules(&lowering_rules);
    let proof_trace_hints = proof_trace_hints_for_rules(&lowering_rules);
    let requires_bridge_lemmas = bridge_lemmas_for_rules(&lowering_rules);

    TranslatorIRMetadata {
        sort: sort.to_string(),
        binders,
        theorem_goal: format!("({}) -> ({})", atom.requires, atom.ensures),
        provenance_span: TranslatorIRProvenanceSpan {
            file: atom.span.file.clone(),
            line: atom.span.line,
            col: atom.span.col,
            len: atom.span.len,
        },
        lowering_rules,
        manual_lemma_reason: None,
        semantic_gap_notes,
        proof_trace_hints,
        requires_bridge_lemmas,
    }
}

fn semantic_gap_notes_for_rules(lowering_rules: &[String]) -> Vec<String> {
    let mut notes = Vec::new();
    if lowering_rules
        .iter()
        .any(|rule| rule == "integer_overflow_bridge")
    {
        notes.push(
            "integer_overflow_bridge: Mumei uses 2's complement wrap semantics, Lean 4 Int is unbounded. Bridge lemma required for overflow behavior."
                .to_string(),
        );
    }
    if lowering_rules
        .iter()
        .any(|rule| rule == "array_bounds_bridge")
    {
        notes.push(
            "array_bounds_bridge: Mumei requires explicit bounds checking, Lean 4 guarded List access requires bounds evidence."
                .to_string(),
        );
    }
    if lowering_rules
        .iter()
        .any(|rule| rule == "string_regex_bridge")
    {
        notes.push(
            "string_regex_bridge: String operations and regex semantics differ between Z3 and Lean 4. Manual lemma may be required."
                .to_string(),
        );
    }
    if lowering_rules
        .iter()
        .any(|rule| rule == "refinement_predicate_lowering")
    {
        notes.push(
            "refinement_predicate_lowering: Quantifiers and refinement predicates are lowered to Lean dependent types."
                .to_string(),
        );
    }
    if lowering_rules
        .iter()
        .any(|rule| rule == "finite_field_lowering")
    {
        notes.push(
            "finite_field_lowering: GF(p)-style helpers are lowered through MumeiLean.Algebra helpers."
                .to_string(),
        );
    }
    notes
}

fn proof_trace_hints_for_rules(lowering_rules: &[String]) -> Vec<String> {
    let mut hints = Vec::new();
    if lowering_rules
        .iter()
        .any(|rule| rule == "integer_overflow_bridge")
    {
        hints.push(
            "assert mumei_i64_in_range hypotheses before applying arithmetic lemmas".to_string(),
        );
    }
    if lowering_rules
        .iter()
        .any(|rule| rule == "array_bounds_bridge")
    {
        hints.push("preserve i < arr.length evidence before guarded List access".to_string());
    }
    if lowering_rules
        .iter()
        .any(|rule| rule == "string_regex_bridge")
    {
        hints
            .push("route regex/string obligations through explicit bridge assumptions".to_string());
    }
    if lowering_rules
        .iter()
        .any(|rule| rule == "refinement_predicate_lowering")
    {
        hints.push("carry subtype predicate witnesses through quantifier lowering".to_string());
    }
    if lowering_rules
        .iter()
        .any(|rule| rule == "finite_field_lowering")
    {
        hints.push(
            "try finite-field equality helpers before falling back to manual proof".to_string(),
        );
    }
    hints
}

fn bridge_lemmas_for_rules(lowering_rules: &[String]) -> Vec<String> {
    let mut lemmas = Vec::new();
    if lowering_rules
        .iter()
        .any(|rule| rule == "integer_overflow_bridge")
    {
        lemmas.push("mumei_i64_overflow_bridge".to_string());
        lemmas.push("mumei_i64_add_overflow_bridge".to_string());
        lemmas.push("mumei_div_by_zero_bridge".to_string());
    }
    if lowering_rules
        .iter()
        .any(|rule| rule == "array_bounds_bridge")
    {
        lemmas.push("mumei_array_bounds_bridge".to_string());
        lemmas.push("mumei_array_get_bridge".to_string());
    }
    if lowering_rules
        .iter()
        .any(|rule| rule == "string_regex_bridge")
    {
        lemmas.push("mumei_regex_bridge".to_string());
        lemmas.push("mumei_string_concat_bridge".to_string());
    }
    if lowering_rules
        .iter()
        .any(|rule| rule == "refinement_predicate_lowering")
    {
        lemmas.push("mumei_subtype_predicate_bridge".to_string());
    }
    if lowering_rules
        .iter()
        .any(|rule| rule == "finite_field_lowering")
    {
        lemmas.push("mumei_finite_field_bridge".to_string());
    }
    lemmas
}

pub(crate) fn lean_binder_name(name: &str) -> String {
    let mut result = String::new();
    for (idx, ch) in name.chars().enumerate() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if idx == 0 && ch.is_ascii_digit() {
                result.push('_');
            }
            result.push(ch);
        } else {
            result.push('_');
        }
    }
    if result.is_empty() || is_lean_reserved_word(&result) {
        format!("{}_binder", result)
    } else {
        result
    }
}

pub(crate) fn is_lean_reserved_word(name: &str) -> bool {
    matches!(
        name,
        "by" | "def" | "end" | "fun" | "have" | "if" | "let" | "match" | "namespace" | "theorem"
    )
}

fn format_lowered_type_to_lean(lowered: &LoweredType) -> String {
    match lowered {
        LoweredType::I64 => "Int".to_string(),
        LoweredType::I32 => "i32".to_string(),
        LoweredType::U64 => "u64".to_string(),
        LoweredType::U32 => "u32".to_string(),
        LoweredType::F64 => "f64".to_string(),
        LoweredType::F32 => "f32".to_string(),
        LoweredType::Bool => "Bool".to_string(),
        LoweredType::Str => "String".to_string(),
        LoweredType::Array(inner) => {
            let inner_str = format_lowered_type_to_lean(inner);
            // Lean function application is left-associative, so a compound
            // element type must be parenthesized: `List (List Int)`.
            if matches!(**inner, LoweredType::Array(_)) {
                format!("List ({})", inner_str)
            } else {
                format!("List {}", inner_str)
            }
        }
        LoweredType::Other(name) => match name.as_str() {
            "int" | "Int" => "Int".to_string(),
            "string" | "String" | "Str" => "String".to_string(),
            "unit" | "Unit" => "Unit".to_string(),
            "bool" | "Bool" => "Bool".to_string(),
            other => other.to_string(),
        },
    }
}

pub(crate) fn mumei_type_to_lean_type(type_name: &str) -> String {
    format_lowered_type_to_lean(&lower(type_name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationReason {
    #[serde(
        alias = "z3_timeout_complex_fragment",
        alias = "z3_timeout_or_resource_limit"
    )]
    Z3Timeout,
    #[serde(
        alias = "z3_unknown_complex_fragment",
        alias = "z3_resource_limit",
        alias = "z3_resource_limit_complex_fragment"
    )]
    Z3Unknown,
    #[serde(alias = "outside_decidable_fragment")]
    NonlinearArithmetic,
    QuantifierAlternation,
    #[serde(alias = "spurious_counterexample")]
    SpuriousCandidate,
    #[serde(alias = "trusted_atom_human_review", alias = "manual_review")]
    HumanReviewRequired,
}

impl EscalationReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Z3Timeout => "z3_timeout",
            Self::Z3Unknown => "z3_unknown",
            Self::NonlinearArithmetic => "nonlinear_arithmetic",
            Self::QuantifierAlternation => "quantifier_alternation",
            Self::SpuriousCandidate => "spurious_candidate",
            Self::HumanReviewRequired => "human_review_required",
        }
    }
}

impl std::fmt::Display for EscalationReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogicFragment {
    LinearArithmetic,
    NonlinearArithmetic,
    #[serde(alias = "array_without_bounds")]
    ArrayAccess,
    #[serde(
        alias = "quantified_formula",
        alias = "quantifier_alternation",
        alias = "trigger_sensitive_quantifier"
    )]
    QuantifierAlternation,
    #[serde(alias = "recursive_invariant", alias = "complex_temporal_effect")]
    TemporalState,
    FiniteField,
}

impl LogicFragment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LinearArithmetic => "linear_arithmetic",
            Self::NonlinearArithmetic => "nonlinear_arithmetic",
            Self::ArrayAccess => "array_access",
            Self::QuantifierAlternation => "quantifier_alternation",
            Self::TemporalState => "temporal_state",
            Self::FiniteField => "finite_field",
        }
    }
}

impl std::fmt::Display for LogicFragment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn primary_logic_fragment_tag(tags: &[String]) -> LogicFragment {
    if tags.iter().any(|tag| tag == "nonlinear_arithmetic") {
        LogicFragment::NonlinearArithmetic
    } else if tags
        .iter()
        .any(|tag| tag == "quantifier_alternation" || tag == "trigger_sensitive_quantifier")
    {
        LogicFragment::QuantifierAlternation
    } else if tags
        .iter()
        .any(|tag| tag == "array_without_bounds" || tag == "array_access")
    {
        LogicFragment::ArrayAccess
    } else if tags
        .iter()
        .any(|tag| tag == "recursive_invariant" || tag == "complex_temporal_effect")
    {
        LogicFragment::TemporalState
    } else if tags.iter().any(|tag| tag == "finite_field") {
        LogicFragment::FiniteField
    } else {
        LogicFragment::LinearArithmetic
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeanEscalationClassification {
    pub z3_result_class: String,
    pub should_escalate: bool,
    pub escalation_reason: Option<EscalationReason>,
    pub logic_fragment_tag: Option<LogicFragment>,
    pub logic_fragment_tags: Vec<String>,
}

pub fn classify_z3_result(result: &str) -> &'static str {
    let normalized = result.to_ascii_lowercase();
    if normalized.contains("timeout") {
        "timeout"
    } else if normalized.contains("resource") || normalized.contains("budget") {
        "resource_limit"
    } else if normalized.contains("unknown") {
        "unknown"
    } else if normalized == "unsat"
        || normalized.starts_with("unsat ")
        || normalized.starts_with("unsat_core=")
        || normalized.contains("proven")
    {
        "unsat"
    } else if normalized.contains("skipped") {
        "skipped"
    } else if normalized.contains("spurious_candidate")
        || normalized.contains("spurious counterexample")
        || normalized.contains("sat")
        || normalized.contains("counter")
    {
        "sat"
    } else {
        "skipped"
    }
}

pub fn z3_result_from_error_message(message: &str) -> Option<&'static str> {
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("z3 returned unknown") || normalized.contains("solver returned unknown")
    {
        Some("unknown")
    } else if normalized.contains("timeout") {
        Some("timeout")
    } else if normalized.contains("resource limit")
        || normalized.contains("constraint budget exceeded")
    {
        Some("resource_limit")
    } else if normalized.contains("spurious_candidate")
        || normalized.contains("spurious counterexample")
        || normalized.contains("does not match mumei body result")
        || normalized.contains("does not violate ensures under mumei semantics")
    {
        Some("spurious_candidate")
    } else {
        None
    }
}

pub fn classify_atom_for_lean_escalation(
    atom: &Atom,
    module_env: &ModuleEnv,
    z3_result: &str,
    status: &str,
) -> LeanEscalationClassification {
    let z3_result_class = classify_z3_result(z3_result).to_string();
    let logic_fragment_tags = detect_logic_fragment_tags(atom, module_env);
    let logic_fragment_tag = Some(primary_logic_fragment_tag(&logic_fragment_tags));
    let normalized_result = z3_result.to_ascii_lowercase();
    let normalized_status = status.to_ascii_lowercase();

    let mut reason = None;
    if normalized_result.contains("partial_translation")
        || normalized_status.contains("partial_translation")
        || normalized_result.contains("requires_unsat")
        || normalized_status.contains("requires_unsat")
        || normalized_status.contains("spec_contradiction")
    {
        reason = None;
    } else if normalized_result.contains("spurious_candidate")
        || normalized_status.contains("spurious_candidate")
    {
        reason = Some(EscalationReason::SpuriousCandidate);
    } else if matches!(
        z3_result_class.as_str(),
        "unknown" | "timeout" | "resource_limit"
    ) {
        reason = Some(match z3_result_class.as_str() {
            "timeout" => EscalationReason::Z3Timeout,
            _ if matches!(logic_fragment_tag, Some(LogicFragment::NonlinearArithmetic)) => {
                EscalationReason::NonlinearArithmetic
            }
            _ if matches!(
                logic_fragment_tag,
                Some(LogicFragment::QuantifierAlternation)
            ) =>
            {
                EscalationReason::QuantifierAlternation
            }
            _ => EscalationReason::Z3Unknown,
        });
    } else if is_outside_decidable_fragment(&logic_fragment_tags)
        && z3_result_class != "sat"
        && !normalized_status.contains("failed")
    {
        reason = Some(match logic_fragment_tag {
            Some(LogicFragment::QuantifierAlternation) => EscalationReason::QuantifierAlternation,
            _ => EscalationReason::NonlinearArithmetic,
        });
    } else if atom.trust_level == TrustLevel::Trusted {
        reason = Some(EscalationReason::HumanReviewRequired);
    }

    LeanEscalationClassification {
        z3_result_class,
        should_escalate: reason.is_some(),
        escalation_reason: reason,
        logic_fragment_tag,
        logic_fragment_tags,
    }
}

pub(crate) type Env<'a> = HashMap<String, Dynamic<'a>>;
pub(crate) type DynResult<'a> = MumeiResult<Dynamic<'a>>;

#[cfg(test)]
mod tests {
    use super::mumei_type_to_lean_type;

    #[test]
    fn test_mumei_type_to_lean_type_array_and_alias_edges() {
        assert_eq!(mumei_type_to_lean_type("[[i64]]"), "List (List Int)");
        assert_eq!(mumei_type_to_lean_type("unit"), "Unit");
        assert_eq!(mumei_type_to_lean_type("[]<i64>"), "List Int");
    }
}
