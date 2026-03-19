//! # LSP モジュール
//!
//! `mumei lsp` コマンドの実装。
//! JSON-RPC over stdio で Language Server Protocol を提供する。
//!
//! ## 対応機能（Phase 1: 最小実装）
//! - `initialize` / `initialized` ハンドシェイク
//! - `textDocument/didOpen` / `textDocument/didChange` → パースして diagnostics 送信
//! - `shutdown` / `exit`
//!
//! ## 将来の拡張（Phase 2+）
//! - `textDocument/hover` — atom の requires/ensures 表示
//! - `textDocument/completion` — キーワード・atom 名補完
//! - `textDocument/publishDiagnostics` — Z3 検証エラーのリアルタイム表示
//! - `textDocument/definition` — 定義ジャンプ
use crate::parser;
use crate::verification;
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
                        "completionProvider": null
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
    let items = crate::parser::parse_module(source);
    if items.is_empty() {
        return Ok(());
    }

    let mut module_env = verification::ModuleEnv::new();
    verification::register_builtin_traits(&mut module_env);
    verification::register_builtin_effects(&mut module_env);

    // mumei.toml を探してプロジェクトルートを決定
    let base_dir = path.parent().unwrap_or(std::path::Path::new("."));
    let _ = crate::resolver::resolve_prelude(base_dir, &mut module_env);

    // mumei.toml があれば依存パッケージも解決（ジャンプ先の定義が利用可能になる）
    if let Some((proj_dir, manifest)) = crate::manifest::find_and_load() {
        let _ =
            crate::resolver::resolve_manifest_dependencies(&manifest, &proj_dir, &mut module_env);
    }

    let _ = crate::resolver::resolve_imports(&items, base_dir, &mut module_env);

    for item in &items {
        match item {
            crate::parser::Item::TypeDef(t) => module_env.register_type(t),
            crate::parser::Item::StructDef(s) => module_env.register_struct(s),
            crate::parser::Item::EnumDef(e) => module_env.register_enum(e),
            crate::parser::Item::Atom(a) => module_env.register_atom(a),
            crate::parser::Item::TraitDef(t) => module_env.register_trait(t),
            crate::parser::Item::ImplDef(i) => module_env.register_impl(i),
            crate::parser::Item::ResourceDef(r) => module_env.register_resource(r),
            crate::parser::Item::Import(_) => {}
            crate::parser::Item::ExternBlock(_) => {}
            crate::parser::Item::EffectDef(e) => module_env.register_effect(e),
        }
    }

    let output_dir = std::path::Path::new(".");
    for item in &items {
        if let crate::parser::Item::Atom(atom) = item {
            if module_env.is_verified(&atom.name) {
                continue;
            }
            let hir_atom = crate::hir::lower_atom_to_hir(atom);
            verification::verify_with_config(&hir_atom, output_dir, &module_env, 5000, 3)?;
            module_env.mark_verified(&atom.name);
        }
    }

    Ok(())
}

/// Feature 3f: Extract related diagnostic information from MumeiError for LSP relatedInformation.
/// Maps RelatedDiagnostic entries to LSP DiagnosticRelatedInformation format.
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
            // Convert byte offset from SourceSpan to 0-indexed line/character
            // by scanning the source text stored in the RelatedDiagnostic.
            let source_text = r.src.inner().as_str();
            let byte_offset = r.span.offset();
            let (line, character) = byte_offset_to_line_col(source_text, byte_offset);
            // Use the RelatedDiagnostic's own file name when available,
            // falling back to the primary diagnostic's URI for same-file spans.
            let related_uri = {
                let name = r.src.name();
                if !name.is_empty() && name != "<unknown>" {
                    format!("file://{}", name)
                } else {
                    uri.to_string()
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
    let items = crate::parser::parse_module(source);
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
            if let crate::parser::Item::Atom(a) = it {
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
