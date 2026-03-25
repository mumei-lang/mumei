//! # LSP モジュール
//!
//! `mumei lsp` コマンドの実装。
//! JSON-RPC over stdio で Language Server Protocol を提供する。
//!
//! ## 対応機能
//! - `initialize` / `initialized` ハンドシェイク
//! - `textDocument/didOpen` / `textDocument/didChange` → パースして diagnostics 送信
//! - `textDocument/hover` — atom の requires/ensures 表示
//! - `textDocument/completion` — キーワード・atom・effect・type 名補完
//! - `textDocument/definition` — 定義ジャンプ
//! - `textDocument/publishDiagnostics` — Z3 検証エラーのリアルタイム表示
//! - `shutdown` / `exit`
use mumei_core::parser;
use mumei_core::verification;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
// =============================================================================
// メイン処理
// =============================================================================
/// `mumei lsp` のエントリポイント — stdio で JSON-RPC メッセージを処理
pub fn run() {
    eprintln!("mumei-lsp: starting (stdio mode)...");
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    // ファイル URI → ソースコード のキャッシュ
    let mut documents: HashMap<String, String> = HashMap::new();
    // ファイル URI → パース済みアイテム のキャッシュ（completion/definition 用）
    let mut parsed_items: HashMap<String, Vec<parser::Item>> = HashMap::new();
    loop {
        // LSP メッセージを読み取り
        let message = match read_message(&mut reader) {
            Ok(msg) => msg,
            Err(e) => {
                eprintln!("mumei-lsp: read error: {}", e);
                break;
            }
        };
        // JSON パース
        let json: serde_json::Value = match serde_json::from_str(&message) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("mumei-lsp: JSON parse error: {}", e);
                continue;
            }
        };
        let method = json.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = json.get("id").cloned();
        match method {
            "initialize" => {
                let result = serde_json::json!({
                    "capabilities": {
                        "textDocumentSync": 1,
                        "hoverProvider": true,
                        "completionProvider": {
                            "triggerCharacters": [".", ":"]
                        },
                        "definitionProvider": true
                    },
                    "serverInfo": {
                        "name": "mumei-lsp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                });
                if let Some(id) = id {
                    send_response(&mut writer, id, result);
                }
            }
            "initialized" => {
                eprintln!("mumei-lsp: initialized");
            }
            "textDocument/didOpen" => {
                if let Some(params) = json.get("params") {
                    if let Some(td) = params.get("textDocument") {
                        let uri = td.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                        let text = td.get("text").and_then(|t| t.as_str()).unwrap_or("");
                        documents.insert(uri.to_string(), text.to_string());
                        let items = parser::parse_module(text);
                        parsed_items.insert(uri.to_string(), items);
                        let diagnostics = diagnose(uri, text);
                        send_diagnostics(&mut writer, uri, &diagnostics);
                    }
                }
            }
            "textDocument/didChange" => {
                if let Some(params) = json.get("params") {
                    if let Some(td) = params.get("textDocument") {
                        let uri = td.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                        // contentChanges[0].text (full sync mode)
                        if let Some(changes) =
                            params.get("contentChanges").and_then(|c| c.as_array())
                        {
                            if let Some(change) = changes.first() {
                                if let Some(text) = change.get("text").and_then(|t| t.as_str()) {
                                    documents.insert(uri.to_string(), text.to_string());
                                    let items = parser::parse_module(text);
                                    parsed_items.insert(uri.to_string(), items);
                                    let diagnostics = diagnose(uri, text);
                                    send_diagnostics(&mut writer, uri, &diagnostics);
                                }
                            }
                        }
                    }
                }
            }
            "textDocument/didClose" => {
                if let Some(params) = json.get("params") {
                    if let Some(td) = params.get("textDocument") {
                        let uri = td.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                        documents.remove(uri);
                        parsed_items.remove(uri);
                        // diagnostics をクリア
                        send_diagnostics(&mut writer, uri, &[]);
                    }
                }
            }
            "textDocument/hover" => {
                // 簡易 hover: カーソル行付近の `atom <name>(...)` を探索し、契約を表示
                let hover_result = if let Some(params) = json.get("params") {
                    let uri = params
                        .get("textDocument")
                        .and_then(|td| td.get("uri"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    let line = params
                        .get("position")
                        .and_then(|p| p.get("line"))
                        .and_then(|l| l.as_u64())
                        .unwrap_or(0) as usize;
                    if let Some(text) = documents.get(uri) {
                        build_hover(text, line)
                    } else {
                        None
                    }
                } else {
                    None
                };

                let result = if let Some(contents) = hover_result {
                    serde_json::json!({
                        "contents": {
                            "kind": "markdown",
                            "value": contents
                        }
                    })
                } else {
                    serde_json::Value::Null
                };

                if let Some(id) = id {
                    send_response(&mut writer, id, result);
                }
            }
            "textDocument/completion" => {
                let result = if let Some(params) = json.get("params") {
                    let uri = params
                        .get("textDocument")
                        .and_then(|td| td.get("uri"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    build_completion_list(&parsed_items, uri)
                } else {
                    serde_json::json!([])
                };
                if let Some(id) = id {
                    send_response(&mut writer, id, result);
                }
            }
            "textDocument/definition" => {
                let result = if let Some(params) = json.get("params") {
                    let uri = params
                        .get("textDocument")
                        .and_then(|td| td.get("uri"))
                        .and_then(|u| u.as_str())
                        .unwrap_or("");
                    let line = params
                        .get("position")
                        .and_then(|p| p.get("line"))
                        .and_then(|l| l.as_u64())
                        .unwrap_or(0) as usize;
                    let character = params
                        .get("position")
                        .and_then(|p| p.get("character"))
                        .and_then(|c| c.as_u64())
                        .unwrap_or(0) as usize;
                    if let Some(text) = documents.get(uri) {
                        let word = extract_word_at(text, line, character);
                        if word.is_empty() {
                            serde_json::Value::Null
                        } else {
                            find_definition(&parsed_items, &word)
                        }
                    } else {
                        serde_json::Value::Null
                    }
                } else {
                    serde_json::Value::Null
                };
                if let Some(id) = id {
                    send_response(&mut writer, id, result);
                }
            }
            "shutdown" => {
                eprintln!("mumei-lsp: shutdown requested");
                if let Some(id) = id {
                    send_response(&mut writer, id, serde_json::Value::Null);
                }
            }
            "exit" => {
                eprintln!("mumei-lsp: exit");
                break;
            }
            _ => {
                // 未対応メソッド — リクエストなら MethodNotFound を返す
                if let Some(id) = id {
                    send_error(
                        &mut writer,
                        id,
                        -32601,
                        &format!("Method not found: {}", method),
                    );
                }
            }
        }
    }
}
// =============================================================================
// 診断（パースエラー検出）
// =============================================================================
/// ソースコードをパースして diagnostics を生成
fn diagnose(uri: &str, source: &str) -> Vec<serde_json::Value> {
    // Phase 1: パースできるか
    let items = parser::parse_module(source);
    let mut diagnostics = Vec::new();

    // ソースが空でない場合にアイテムが0個 → パースエラーの可能性
    let trimmed = source.trim();
    if !trimmed.is_empty() && items.is_empty() && !trimmed.starts_with("//") {
        diagnostics.push(serde_json::json!({
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": 0, "character": 1 }
            },
            "severity": 1,
            "source": "mumei",
            "message": "Parse error: no valid items found. Check syntax."
        }));
        return diagnostics;
    }

    // Phase 2: Z3 検証 diagnostics（file:// URI の場合のみ実行）
    if let Some(path) = uri_to_path(uri) {
        if let Err(e) = verify_source_for_lsp(&path, source) {
            let detail = e.to_detail();
            // ErrorDetail の Span から直接位置を取得（substring マッチ不要）
            let (line, col) = if detail.span.line > 0 {
                (
                    detail.span.line.saturating_sub(1),
                    detail.span.col.saturating_sub(1),
                )
            } else {
                // Span が不明な場合、atom の Span にフォールバック
                find_error_position(&items, &detail.message)
            };
            // Feature 3f: Build relatedInformation from MumeiError's #[related] field
            let related_info = build_related_information(uri, &e);
            let mut diag = serde_json::json!({
                "range": {
                    "start": { "line": line, "character": col },
                    "end": { "line": line, "character": col + 1 }
                },
                "severity": 1,
                "source": "mumei-z3",
                "message": format!("{}", detail)
            });
            if !related_info.is_empty() {
                diag["relatedInformation"] = serde_json::json!(related_info);
            }
            diagnostics.push(diag);
        }
    }

    diagnostics
}

/// エラーメッセージから関連する atom の Span 情報を検索し、行・列を返す。
/// マッチしない場合は (0, 0) にフォールバックする。
fn find_error_position(items: &[parser::Item], error_msg: &str) -> (usize, usize) {
    // エラーメッセージに atom 名が含まれている場合、その atom の Span を使用
    // TODO: contains() による部分文字列マッチは短い atom 名で誤マッチする可能性がある。
    //       将来的には ErrorDetail.span を直接使用するか、ワード境界チェックを追加すべき。
    for item in items {
        if let parser::Item::Atom(atom) = item {
            if error_msg.contains(&atom.name) && atom.span.line > 0 {
                // LSP は 0-indexed なので 1-indexed の Span から変換
                return (
                    atom.span.line.saturating_sub(1),
                    atom.span.col.saturating_sub(1),
                );
            }
        }
    }
    (0, 0)
}

fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    uri.strip_prefix("file://").map(std::path::PathBuf::from)
}

/// ソースコードを in-process でパース → Z3 検証し、最初のエラーを返す。
/// mumei.toml を上方探索してプロジェクトルートを決定し、依存パッケージも解決する。
fn verify_source_for_lsp(
    path: &std::path::Path,
    source: &str,
) -> Result<(), verification::MumeiError> {
    let items = parser::parse_module(source);
    if items.is_empty() {
        return Ok(());
    }

    let mut module_env = verification::ModuleEnv::new();
    verification::register_builtin_traits(&mut module_env);
    verification::register_builtin_effects(&mut module_env);

    // mumei.toml を探してプロジェクトルートを決定
    let base_dir = path.parent().unwrap_or(std::path::Path::new("."));
    let _ = mumei_core::resolver::resolve_prelude(base_dir, &mut module_env);

    // mumei.toml があれば依存パッケージも解決（ジャンプ先の定義が利用可能になる）
    if let Some((proj_dir, manifest)) = mumei_core::manifest::find_and_load() {
        let _ = mumei_core::resolver::resolve_manifest_dependencies(
            &manifest,
            &proj_dir,
            &mut module_env,
        );
    }

    let _ = mumei_core::resolver::resolve_imports(&items, base_dir, &mut module_env);

    for item in &items {
        match item {
            parser::Item::TypeDef(t) => module_env.register_type(t),
            parser::Item::StructDef(s) => module_env.register_struct(s),
            parser::Item::EnumDef(e) => module_env.register_enum(e),
            parser::Item::Atom(a) => module_env.register_atom(a),
            parser::Item::TraitDef(t) => module_env.register_trait(t),
            parser::Item::ImplDef(i) => module_env.register_impl(i),
            parser::Item::ResourceDef(r) => module_env.register_resource(r),
            parser::Item::Import(_) => {}
            parser::Item::ExternBlock(_) => {}
            parser::Item::EffectDef(e) => module_env.register_effect(e),
            parser::Item::ImplBlock(ib) => {
                for method in &ib.methods {
                    let mut qualified = method.clone();
                    qualified.name = format!("{}::{}", ib.struct_name, method.name);
                    module_env.register_atom(&qualified);
                }
            }
        }
    }

    let output_dir = std::path::Path::new(".");
    for item in &items {
        match item {
            parser::Item::Atom(atom) => {
                if module_env.is_verified(&atom.name) {
                    continue;
                }
                let hir_atom = mumei_core::hir::lower_atom_to_hir(atom);
                verification::verify_with_config(&hir_atom, output_dir, &module_env, 5000, 3)?;
                module_env.mark_verified(&atom.name);
            }
            parser::Item::ImplBlock(ib) => {
                for method in &ib.methods {
                    let qualified_name = format!("{}::{}", ib.struct_name, method.name);
                    if module_env.is_verified(&qualified_name) {
                        continue;
                    }
                    let mut qualified_method = method.clone();
                    qualified_method.name = qualified_name.clone();
                    let hir_atom = mumei_core::hir::lower_atom_to_hir(&qualified_method);
                    verification::verify_with_config(&hir_atom, output_dir, &module_env, 5000, 3)?;
                    module_env.mark_verified(&qualified_name);
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// Feature 3f: Extract related diagnostic information from MumeiError for LSP relatedInformation.
/// Maps RelatedDiagnostic entries to LSP DiagnosticRelatedInformation format.
///
/// Uses `RelatedDiagnostic.original_span` (parser::Span with 1-indexed line/col) for precise
/// positioning. This works even when source text is empty (e.g., in the LSP path where
/// `with_source()` has not been called), because line/col are resolved directly from the
/// parser Span without needing to scan source text.
fn build_related_information(
    uri: &str,
    error: &verification::MumeiError,
) -> Vec<serde_json::Value> {
    let related_spans = match error {
        verification::MumeiError::VerificationError { related, .. }
        | verification::MumeiError::CodegenError { related, .. }
        | verification::MumeiError::TypeError { related, .. } => related,
    };
    related_spans
        .iter()
        .map(|r| {
            // Use original_span (parser::Span) for line/col resolution.
            // parser::Span is 1-indexed; LSP expects 0-indexed.
            let (line, character) = if r.original_span.line > 0 {
                (
                    r.original_span.line.saturating_sub(1),
                    r.original_span.col.saturating_sub(1),
                )
            } else {
                // Fallback: try byte offset conversion from source text
                let source_text = r.src.inner().as_str();
                let byte_offset = r.span.offset();
                byte_offset_to_line_col(source_text, byte_offset)
            };
            // Use the RelatedDiagnostic's own file name when available,
            // falling back to the primary diagnostic's URI for same-file spans.
            let related_uri = {
                let name = r.original_span.file.as_str();
                if !name.is_empty() {
                    format!("file://{}", name)
                } else {
                    let src_name = r.src.name();
                    if !src_name.is_empty() && src_name != "<unknown>" {
                        format!("file://{}", src_name)
                    } else {
                        uri.to_string()
                    }
                }
            };
            serde_json::json!({
                "location": {
                    "uri": related_uri,
                    "range": {
                        "start": { "line": line, "character": character },
                        "end": { "line": line, "character": character + 1 }
                    }
                },
                "message": format!("{}: {}", r.label, r.msg)
            })
        })
        .collect()
}

/// Convert a byte offset within `source` to a 0-indexed (line, character) pair.
/// Falls back to (0, 0) when the source is empty or the offset is out of range.
fn byte_offset_to_line_col(source: &str, byte_offset: usize) -> (usize, usize) {
    let mut line: usize = 0;
    let mut col: usize = 0;
    for (idx, ch) in source.char_indices() {
        if idx >= byte_offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Hover 用: 指定行付近の atom を探し、requires/ensures を markdown で返す
fn build_hover(source: &str, line: usize) -> Option<String> {
    let items = parser::parse_module(source);
    let lines: Vec<&str> = source.lines().collect();
    let target_line = lines.get(line).copied().unwrap_or("");

    // 1) その行に atom 名が書かれているケース: `atom name(`
    let atom_name = if let Some(idx) = target_line.find("atom ") {
        let rest = &target_line[idx + 5..];
        rest.split(|c: char| c == '(' || c.is_whitespace())
            .next()
            .map(|s| s.trim().to_string())
    } else {
        None
    };

    // 2) パース済み items から契約を拾う
    if let Some(name) = atom_name {
        for it in &items {
            if let parser::Item::Atom(a) = it {
                if a.name == name {
                    let mut md = format!(
                        "### atom {}\n\n**requires**:\n```\n{}\n```\n\n**ensures**:\n```\n{}\n```",
                        a.name,
                        a.requires.trim(),
                        a.ensures.trim()
                    );
                    // エフェクト情報を表示
                    if !a.effects.is_empty() {
                        let effects_str: Vec<String> =
                            a.effects.iter().map(|e| e.name.clone()).collect();
                        md.push_str(&format!("\n\n**effects**: `[{}]`", effects_str.join(", ")));
                    }
                    return Some(md);
                }
            }
        }
    }

    None
}

// =============================================================================
// Completion (textDocument/completion)
// =============================================================================

/// All mumei keywords (from mumei-core/src/parser/lexer.rs read_identifier)
const MUMEI_KEYWORDS: &[&str] = &[
    "atom",
    "atom_ref",
    "task_group",
    "task",
    "max_unroll",
    "let",
    "if",
    "else",
    "while",
    "match",
    "fn",
    "struct",
    "enum",
    "trait",
    "impl",
    "import",
    "type",
    "where",
    "requires",
    "ensures",
    "body",
    "true",
    "false",
    "trusted",
    "unverified",
    "async",
    "await",
    "acquire",
    "resource",
    "effect",
    "extern",
    "consume",
    "invariant",
    "decreases",
    "effects",
    "resources",
    "for",
    "forall",
    "exists",
    "ref",
    "mut",
    "call",
    "perform",
    "law",
    "priority",
    "mode",
    "exclusive",
    "shared",
    "includes",
    "parent",
    "as",
    "contract",
    "chan",
    "send",
    "recv",
    "cancel",
];

/// Build a CompletionItem list from keywords + cached parsed items.
fn build_completion_list(
    parsed_items: &HashMap<String, Vec<parser::Item>>,
    _current_uri: &str,
) -> serde_json::Value {
    let mut items: Vec<serde_json::Value> = Vec::new();

    // 1) Keywords (kind = 14)
    for kw in MUMEI_KEYWORDS {
        items.push(serde_json::json!({
            "label": kw,
            "kind": 14
        }));
    }

    // 2) Names from parsed items across all open documents
    for doc_items in parsed_items.values() {
        for item in doc_items {
            match item {
                parser::Item::Atom(a) => {
                    items.push(serde_json::json!({
                        "label": a.name,
                        "kind": 3,
                        "detail": format!(
                            "atom {}  requires: {}  ensures: {}",
                            a.name,
                            a.requires.trim(),
                            a.ensures.trim()
                        )
                    }));
                }
                parser::Item::EffectDef(e) => {
                    items.push(serde_json::json!({
                        "label": e.name,
                        "kind": 8
                    }));
                }
                parser::Item::TypeDef(t) => {
                    items.push(serde_json::json!({
                        "label": t.name,
                        "kind": 7
                    }));
                }
                parser::Item::StructDef(s) => {
                    items.push(serde_json::json!({
                        "label": s.name,
                        "kind": 7
                    }));
                }
                parser::Item::EnumDef(e) => {
                    items.push(serde_json::json!({
                        "label": e.name,
                        "kind": 7
                    }));
                }
                parser::Item::TraitDef(t) => {
                    items.push(serde_json::json!({
                        "label": t.name,
                        "kind": 8
                    }));
                }
                parser::Item::ResourceDef(r) => {
                    items.push(serde_json::json!({
                        "label": r.name,
                        "kind": 7
                    }));
                }
                _ => {}
            }
        }
    }

    serde_json::json!(items)
}

// =============================================================================
// Definition (textDocument/definition)
// =============================================================================

/// Extract the identifier word at a given (0-indexed) line and character position.
fn extract_word_at(source: &str, line: usize, character: usize) -> String {
    let target_line = match source.lines().nth(line) {
        Some(l) => l,
        None => return String::new(),
    };
    let chars: Vec<char> = target_line.chars().collect();
    if character >= chars.len() {
        return String::new();
    }
    // Only extract a word if the cursor is on an identifier character
    if !chars[character].is_alphanumeric() && chars[character] != '_' {
        return String::new();
    }
    // Scan backward to find word start
    let mut start = character;
    while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
        start -= 1;
    }
    // Scan forward to find word end
    let mut end = character;
    while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
        end += 1;
    }
    if start == end {
        return String::new();
    }
    chars[start..end].iter().collect()
}

/// Search all cached parsed items for a definition matching the given name.
/// Returns a Location JSON value or Null if not found.
fn find_definition(
    parsed_items: &HashMap<String, Vec<parser::Item>>,
    name: &str,
) -> serde_json::Value {
    for (uri, doc_items) in parsed_items {
        for item in doc_items {
            let (item_name, span) = match item {
                parser::Item::Atom(a) => (&a.name, &a.span),
                parser::Item::TypeDef(t) => (&t.name, &t.span),
                parser::Item::StructDef(s) => (&s.name, &s.span),
                parser::Item::EnumDef(e) => (&e.name, &e.span),
                parser::Item::EffectDef(e) => (&e.name, &e.span),
                parser::Item::TraitDef(t) => (&t.name, &t.span),
                parser::Item::ResourceDef(r) => (&r.name, &r.span),
                _ => continue,
            };
            if item_name == name {
                // Span is 1-indexed; LSP expects 0-indexed
                let def_line = span.line.saturating_sub(1);
                let def_col = span.col.saturating_sub(1);
                // Use the span's file if it has a path, otherwise use the document URI
                let def_uri = if !span.file.is_empty() {
                    format!("file://{}", span.file)
                } else {
                    uri.clone()
                };
                return serde_json::json!({
                    "uri": def_uri,
                    "range": {
                        "start": { "line": def_line, "character": def_col },
                        "end": { "line": def_line, "character": def_col + span.len }
                    }
                });
            }
        }
    }
    serde_json::Value::Null
}

// =============================================================================
// LSP JSON-RPC I/O
// =============================================================================
/// LSP メッセージを stdin から読み取る（Content-Length ヘッダ付き）
fn read_message(reader: &mut impl BufRead) -> Result<String, String> {
    // ヘッダを読み取り
    let mut content_length: usize = 0;
    loop {
        let mut header_line = String::new();
        reader
            .read_line(&mut header_line)
            .map_err(|e| format!("Failed to read header: {}", e))?;
        let trimmed = header_line.trim();
        if trimmed.is_empty() {
            break; // ヘッダ終了（空行）
        }
        if let Some(len_str) = trimmed.strip_prefix("Content-Length: ") {
            content_length = len_str
                .parse::<usize>()
                .map_err(|e| format!("Invalid Content-Length: {}", e))?;
        }
        // Content-Type 等は無視
    }
    if content_length == 0 {
        return Err("Content-Length is 0 or missing".to_string());
    }
    // ボディを読み取り
    let mut body = vec![0u8; content_length];
    reader
        .read_exact(&mut body)
        .map_err(|e| format!("Failed to read body: {}", e))?;
    String::from_utf8(body).map_err(|e| format!("Invalid UTF-8 in body: {}", e))
}
/// JSON-RPC レスポンスを送信
fn send_response(writer: &mut impl Write, id: serde_json::Value, result: serde_json::Value) {
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    });
    send_message(writer, &response);
}
/// JSON-RPC エラーレスポンスを送信
fn send_error(writer: &mut impl Write, id: serde_json::Value, code: i32, message: &str) {
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    });
    send_message(writer, &response);
}
/// textDocument/publishDiagnostics 通知を送信
fn send_diagnostics(writer: &mut impl Write, uri: &str, diagnostics: &[serde_json::Value]) {
    let notification = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {
            "uri": uri,
            "diagnostics": diagnostics
        }
    });
    send_message(writer, &notification);
}
/// LSP メッセージを stdout に送信（Content-Length ヘッダ付き）
fn send_message(writer: &mut impl Write, message: &serde_json::Value) {
    let body = serde_json::to_string(message).unwrap_or_default();
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    let _ = writer.write_all(header.as_bytes());
    let _ = writer.write_all(body.as_bytes());
    let _ = writer.flush();
}

// =============================================================================
// Tests
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_includes_keywords() {
        let parsed_items: HashMap<String, Vec<parser::Item>> = HashMap::new();
        let result = build_completion_list(&parsed_items, "file:///test.mm");
        let items = result.as_array().expect("should be array");
        let labels: Vec<&str> = items
            .iter()
            .filter_map(|i| i.get("label").and_then(|l| l.as_str()))
            .collect();
        assert!(labels.contains(&"atom"));
        assert!(labels.contains(&"requires"));
        assert!(labels.contains(&"ensures"));
        assert!(labels.contains(&"effect"));
        assert!(labels.contains(&"struct"));
        assert!(labels.contains(&"import"));
        // Check keyword kind = 14
        let atom_item = items
            .iter()
            .find(|i| i.get("label").and_then(|l| l.as_str()) == Some("atom"))
            .unwrap();
        assert_eq!(atom_item.get("kind").and_then(|k| k.as_u64()), Some(14));
    }

    #[test]
    fn test_completion_includes_atoms_from_cache() {
        let mut parsed_items: HashMap<String, Vec<parser::Item>> = HashMap::new();
        let source = "atom inc(n: i64) requires: n >= 0; ensures: result == n + 1; body: n + 1;";
        let items = parser::parse_module(source);
        assert!(!items.is_empty(), "should parse at least one item");
        parsed_items.insert("file:///test.mm".to_string(), items);

        let result = build_completion_list(&parsed_items, "file:///test.mm");
        let completion_items = result.as_array().expect("should be array");
        let labels: Vec<&str> = completion_items
            .iter()
            .filter_map(|i| i.get("label").and_then(|l| l.as_str()))
            .collect();
        assert!(labels.contains(&"inc"), "should contain atom name 'inc'");
        // Atom kind = 3 (Function)
        let inc_item = completion_items
            .iter()
            .find(|i| i.get("label").and_then(|l| l.as_str()) == Some("inc"))
            .unwrap();
        assert_eq!(inc_item.get("kind").and_then(|k| k.as_u64()), Some(3));
    }

    #[test]
    fn test_extract_word_at_basic() {
        let source = "atom inc(n: i64)";
        assert_eq!(extract_word_at(source, 0, 0), "atom");
        assert_eq!(extract_word_at(source, 0, 5), "inc");
        assert_eq!(extract_word_at(source, 0, 9), "n");
    }

    #[test]
    fn test_extract_word_at_non_identifier() {
        let source = "atom inc(n: i64)";
        // Position 8 is '(' — should return empty, not "inc"
        assert_eq!(extract_word_at(source, 0, 8), "");
        // Position 4 is ' ' — should return empty, not "atom"
        assert_eq!(extract_word_at(source, 0, 4), "");
        // Position 11 is ':' — should return empty, not "n"
        assert_eq!(extract_word_at(source, 0, 11), "");
    }

    #[test]
    fn test_extract_word_at_out_of_bounds() {
        let source = "hello";
        assert_eq!(extract_word_at(source, 1, 0), "");
        assert_eq!(extract_word_at(source, 0, 100), "");
    }

    #[test]
    fn test_find_definition_atom() {
        let mut parsed_items: HashMap<String, Vec<parser::Item>> = HashMap::new();
        let source = "atom inc(n: i64) requires: n >= 0; ensures: result == n + 1; body: n + 1;";
        let items = parser::parse_module(source);
        parsed_items.insert("file:///test.mm".to_string(), items);

        let result = find_definition(&parsed_items, "inc");
        assert!(!result.is_null(), "should find definition for 'inc'");
        assert!(result.get("uri").is_some());
        assert!(result.get("range").is_some());
    }

    #[test]
    fn test_find_definition_not_found() {
        let parsed_items: HashMap<String, Vec<parser::Item>> = HashMap::new();
        let result = find_definition(&parsed_items, "nonexistent");
        assert!(result.is_null());
    }

    #[test]
    fn test_keyword_list_completeness() {
        // Verify MUMEI_KEYWORDS matches the lexer's keyword count (56)
        assert_eq!(
            MUMEI_KEYWORDS.len(),
            56,
            "MUMEI_KEYWORDS count should match lexer keywords, got {}",
            MUMEI_KEYWORDS.len()
        );
        // Verify no duplicates
        let mut seen = std::collections::HashSet::new();
        for kw in MUMEI_KEYWORDS {
            assert!(seen.insert(kw), "Duplicate keyword: {}", kw);
        }
    }
}
