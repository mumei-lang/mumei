use crate::pipeline::*;
#[cfg(test)]
use mumei_core::hir::lower_atom_to_hir;
use mumei_core::{parser, proof_cert};
use std::fs;
use std::path::Path;

pub(crate) fn cmd_doc(input: &str, output_dir: &str, format: &str) {
    // Plan 10 (Task 4E): when --format json is requested stdout must be
    // valid JSON for piping (`mumei doc … --format json | jq …`). Route all
    // human-readable progress/status messages to stderr so stdout carries
    // only the JSON payload. For other formats the behaviour is identical
    // (stderr is still displayed in interactive terminals).
    eprintln!(
        "🗡️  Mumei doc: generating {} documentation for '{}'...",
        format, input
    );

    let input_path = Path::new(input);
    let out_path = Path::new(output_dir);
    let _ = fs::create_dir_all(out_path);

    // 対象ファイルを収集
    let files: Vec<std::path::PathBuf> = if input_path.is_dir() {
        // ディレクトリの場合、再帰的に .mm ファイルを収集
        collect_mm_files(input_path)
    } else {
        vec![input_path.to_path_buf()]
    };

    if files.is_empty() {
        eprintln!("  ❌ No .mm files found in '{}'", input);
        std::process::exit(1);
    }

    eprintln!("  📄 Found {} file(s)", files.len());

    let mut all_docs: Vec<ModuleDoc> = Vec::new();

    for file in &files {
        let source = match read_source_file(file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  ⚠️  Skipping '{}': {}", file.display(), e);
                continue;
            }
        };

        let items = parser::parse_module(&source);
        let module_name = file
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        let mut doc = ModuleDoc {
            name: module_name,
            file_path: file.display().to_string(),
            atoms: Vec::new(),
            types: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            traits: Vec::new(),
        };

        // ソース行からコメントを抽出
        let lines: Vec<&str> = source.lines().collect();

        for item in &items {
            match item {
                parser::Item::Atom(atom) => {
                    let comment = extract_comment_before(&lines, atom.span.line);
                    doc.atoms.push(ItemDoc {
                        name: atom.name.clone(),
                        comment,
                        params: atom
                            .params
                            .iter()
                            .map(|p| {
                                format!("{}: {}", p.name, p.type_name.as_deref().unwrap_or("?"))
                            })
                            .collect(),
                        trust_level: format!("{:?}", atom.trust_level),
                        is_async: atom.is_async,
                        // Plan 10 (Task 4A): expose contract metadata so the
                        // generated docs (HTML / Markdown / JSON) can render
                        // requires/ensures/body/effects alongside each atom.
                        requires: atom.requires.clone(),
                        ensures: atom.ensures.clone(),
                        body_expr: atom.body_expr.clone(),
                        effects: atom.effects.iter().map(|e| e.name.clone()).collect(),
                    });
                }
                parser::Item::TypeDef(t) => {
                    let comment = extract_comment_before(&lines, t.span.line);
                    doc.types.push(TypeDoc {
                        name: t.name.clone(),
                        comment,
                    });
                }
                parser::Item::StructDef(s) => {
                    let comment = extract_comment_before(&lines, s.span.line);
                    doc.structs.push(TypeDoc {
                        name: s.name.clone(),
                        comment,
                    });
                }
                parser::Item::EnumDef(e) => {
                    let comment = extract_comment_before(&lines, e.span.line);
                    doc.enums.push(TypeDoc {
                        name: e.name.clone(),
                        comment,
                    });
                }
                parser::Item::TraitDef(t) => {
                    let comment = extract_comment_before(&lines, t.span.line);
                    doc.traits.push(TypeDoc {
                        name: t.name.clone(),
                        comment,
                    });
                }
                _ => {}
            }
        }

        all_docs.push(doc);
    }

    // ドキュメント出力
    match format {
        "html" => generate_html_docs(&all_docs, out_path),
        "markdown" | "md" => generate_markdown_docs(&all_docs, out_path),
        "json" => {
            // Plan 10 (Task 4E): structured documentation output for tooling.
            // Emits the full set of contract metadata for every atom so that
            // downstream consumers (LSP, web frontends, code generators) can
            // ingest the docs without re-parsing Mumei source.
            let harness_metadata = collect_harness_doc_metadata();
            let json_docs: Vec<serde_json::Value> = all_docs
                .iter()
                .map(|doc| {
                    let mut module = serde_json::json!({
                        "name": doc.name,
                        "file_path": doc.file_path,
                        "atoms": doc.atoms.iter().map(|a| serde_json::json!({
                            "name": a.name,
                            "comment": a.comment,
                            "params": a.params,
                            "trust_level": a.trust_level,
                            "is_async": a.is_async,
                            "requires": a.requires,
                            "ensures": a.ensures,
                            "body_expr": a.body_expr,
                            "effects": a.effects,
                        })).collect::<Vec<_>>(),
                        "types": doc.types.iter().map(|t| serde_json::json!({
                            "name": t.name,
                            "comment": t.comment,
                        })).collect::<Vec<_>>(),
                        "structs": doc.structs.iter().map(|s| serde_json::json!({
                            "name": s.name,
                            "comment": s.comment,
                        })).collect::<Vec<_>>(),
                        "enums": doc.enums.iter().map(|e| serde_json::json!({
                            "name": e.name,
                            "comment": e.comment,
                        })).collect::<Vec<_>>(),
                        "traits": doc.traits.iter().map(|t| serde_json::json!({
                            "name": t.name,
                            "comment": t.comment,
                        })).collect::<Vec<_>>(),
                    });
                    if let serde_json::Value::Object(ref mut fields) = module {
                        if let Some(ref harness_contract) = harness_metadata.harness_contract {
                            fields.insert(
                                "harness_contract".to_string(),
                                serde_json::Value::String(harness_contract.clone()),
                            );
                        }
                        if let Some(ref intent_fidelity) = harness_metadata.intent_fidelity {
                            fields.insert(
                                "intent_fidelity".to_string(),
                                serde_json::json!(intent_fidelity),
                            );
                        }
                        if let Some(ref artifact_paths) = harness_metadata.artifact_paths {
                            fields.insert(
                                "artifact_paths".to_string(),
                                serde_json::json!(artifact_paths),
                            );
                        }
                        if let Some(ref budget_policy_fingerprint) =
                            harness_metadata.budget_policy_fingerprint
                        {
                            fields.insert(
                                "budget_policy_fingerprint".to_string(),
                                serde_json::Value::String(budget_policy_fingerprint.clone()),
                            );
                        }
                    }
                    module
                })
                .collect();
            let payload =
                serde_json::to_string_pretty(&json_docs).unwrap_or_else(|_| "[]".to_string());
            // Both stream to stdout (for piping) and write to <out>/docs.json
            // so the same invocation works for CLI scripting and static hosting.
            println!("{}", payload);
            let _ = fs::write(out_path.join("docs.json"), payload);
        }
        _ => {
            eprintln!(
                "  ❌ Unknown format '{}'. Use 'html', 'markdown', or 'json'.",
                format
            );
            std::process::exit(1);
        }
    }

    eprintln!("  ✅ Documentation generated in '{}'", out_path.display());
}

// --- doc ヘルパー構造体 ---

pub(crate) struct ModuleDoc {
    name: String,
    file_path: String,
    atoms: Vec<ItemDoc>,
    types: Vec<TypeDoc>,
    structs: Vec<TypeDoc>,
    enums: Vec<TypeDoc>,
    traits: Vec<TypeDoc>,
}

pub(crate) struct ItemDoc {
    name: String,
    comment: String,
    params: Vec<String>,
    trust_level: String,
    is_async: bool,
    /// `requires` clause source text, e.g. `"len > 0 && idx < len"`.
    requires: String,
    /// `ensures` clause source text.
    ensures: String,
    /// Body expression source text, used for browser-side display.
    body_expr: String,
    /// Declared effect names (e.g. `["FileRead"]`).
    effects: Vec<String>,
}

pub(crate) struct HarnessDocMetadata {
    harness_contract: Option<String>,
    intent_fidelity: Option<proof_cert::IntentFidelity>,
    artifact_paths: Option<Vec<String>>,
    budget_policy_fingerprint: Option<String>,
}

pub(crate) fn collect_harness_doc_metadata() -> HarnessDocMetadata {
    HarnessDocMetadata {
        harness_contract: proof_cert::harness_contract_from_env(),
        intent_fidelity: proof_cert::intent_fidelity_from_env(),
        artifact_paths: proof_cert::artifact_paths_from_env(),
        budget_policy_fingerprint: proof_cert::budget_policy_fingerprint_from_env(),
    }
}

pub(crate) struct TypeDoc {
    name: String,
    comment: String,
}

/// ソース行から指定行の直前にあるコメント群を抽出する。
///
/// Plan 10 (Task 4B): Prefer `///` doc comments when present and fall back
/// to ordinary `//` comments otherwise. This keeps backward compatibility
/// with existing source files while letting authors opt into Rust-style
/// doc comments for richer output.
pub(crate) fn extract_comment_before(lines: &[&str], target_line: usize) -> String {
    if target_line == 0 || target_line > lines.len() {
        return String::new();
    }

    // First pass: walk upward collecting `///` doc comments only.
    let mut doc_comments = Vec::new();
    let mut i = target_line.saturating_sub(1);
    while i > 0 {
        let prev = i - 1;
        let line = lines[prev].trim();
        if line.starts_with("///") {
            doc_comments.push(line.trim_start_matches("///").trim().to_string());
            i = prev;
        } else {
            break;
        }
    }
    if !doc_comments.is_empty() {
        doc_comments.reverse();
        return doc_comments.join("\n");
    }

    // Fallback: any `//` comment block immediately above the item.
    let mut comments = Vec::new();
    let mut i = target_line.saturating_sub(1);
    loop {
        if i == 0 && !lines[0].trim().starts_with("//") {
            break;
        }
        if i > 0 {
            let prev = i - 1;
            let line = lines[prev].trim();
            if line.starts_with("//") {
                comments.push(line.trim_start_matches("//").trim().to_string());
                i = prev;
            } else {
                break;
            }
        } else {
            break;
        }
    }
    comments.reverse();
    comments.join("\n")
}

pub(crate) fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Plan 10 (Task 4D): minimal token-level highlighter for Mumei source.
///
/// This is intentionally not a full parser — it just decorates the most
/// recognizable lexical classes (keywords, built-in types, operators,
/// numeric literals, string literals, line comments) so the generated
/// HTML docs render contracts and bodies with readable color cues.
///
/// The implementation runs a single tokenizing pass over the (unescaped)
/// source so we never accidentally match operator characters inside
/// already-emitted HTML such as `class="kw"`. Each matched token is
/// individually HTML-escaped and wrapped in a `<span class="…">`; the
/// gaps between matches are escaped verbatim.
pub(crate) fn highlight_mumei_code(code: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;

    static TOKEN: OnceLock<Regex> = OnceLock::new();

    let token = TOKEN.get_or_init(|| {
        Regex::new(concat!(
            // line comments
            r"(?P<cm>//[^\n]*)",
            r"|",
            // strings
            r#"(?P<str>"(?:[^"\\]|\\.)*")"#,
            r"|",
            // keywords (word boundary)
            r"(?P<kw>\b(?:atom|struct|trait|impl|enum|extern|import|match|if|else|let|mut|return|while|for|in|async|await|unverified|trusted|verified|requires|ensures|body|invariant|where|effects|effect_pre|effect_post|resource|acquire|release|fn|true|false|consume|ref|perform)\b)",
            r"|",
            // builtin types
            r"(?P<ty>\b(?:i32|i64|u32|u64|f32|f64|bool|String|Str|Self)\b)",
            r"|",
            // numbers
            r"(?P<num>\b\d+(?:\.\d+)?\b)",
            r"|",
            // multi-char operators first, then single-char
            r"(?P<op>&&|\|\||==|!=|>=|<=|=>|->|\|>|\+|-|\*|/|%|>|<|=)",
        ))
        .expect("highlight_mumei_code regex must compile")
    });

    let mut out = String::with_capacity(code.len());
    let mut last_end = 0usize;
    for caps in token.captures_iter(code) {
        let m = caps.get(0).unwrap();
        // Emit the gap before this match, HTML-escaped.
        if m.start() > last_end {
            out.push_str(&html_escape(&code[last_end..m.start()]));
        }
        let (class, text) = if let Some(t) = caps.name("cm") {
            ("cm", t.as_str())
        } else if let Some(t) = caps.name("str") {
            ("str", t.as_str())
        } else if let Some(t) = caps.name("kw") {
            ("kw", t.as_str())
        } else if let Some(t) = caps.name("ty") {
            ("ty", t.as_str())
        } else if let Some(t) = caps.name("num") {
            ("num", t.as_str())
        } else if let Some(t) = caps.name("op") {
            ("op", t.as_str())
        } else {
            ("", m.as_str())
        };
        out.push_str(&format!(
            "<span class=\"{}\">{}</span>",
            class,
            html_escape(text)
        ));
        last_end = m.end();
    }
    if last_end < code.len() {
        out.push_str(&html_escape(&code[last_end..]));
    }
    out
}

/// HTML ドキュメントを生成
pub(crate) fn generate_html_docs(docs: &[ModuleDoc], out_dir: &Path) {
    // index.html
    let mut index = String::from(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Mumei Documentation</title>
<style>
body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; margin: 0; padding: 20px; background: #1a1a2e; color: #e0e0e0; }
.container { max-width: 900px; margin: 0 auto; }
h1 { color: #e94560; border-bottom: 2px solid #e94560; padding-bottom: 10px; }
h2 { color: #0f3460; background: #16213e; padding: 8px 16px; border-radius: 4px; color: #e94560; }
h3 { color: #c0c0c0; }
a { color: #e94560; text-decoration: none; }
a:hover { text-decoration: underline; }
.module-list { list-style: none; padding: 0; }
.module-list li { margin: 8px 0; padding: 8px 16px; background: #16213e; border-radius: 4px; }
.atom { background: #16213e; padding: 12px 16px; margin: 8px 0; border-radius: 6px; border-left: 3px solid #e94560; }
.atom .name { color: #e94560; font-weight: bold; font-size: 1.1em; }
.atom .params { color: #a0a0a0; font-family: monospace; }
.atom .comment { color: #c0c0c0; margin-top: 6px; }
.badge { display: inline-block; padding: 2px 8px; border-radius: 3px; font-size: 0.8em; margin-left: 8px; }
.badge.trusted { background: #0f3460; color: #e94560; }
.badge.verified { background: #1a4040; color: #4ecdc4; }
.badge.async { background: #3d1a60; color: #c084fc; }
.type-def { background: #16213e; padding: 8px 16px; margin: 8px 0; border-radius: 6px; border-left: 3px solid #4ecdc4; }
.contract { font-family: monospace; padding: 4px 16px; margin: 2px 0; font-size: 0.9em; }
.contract .label { font-weight: bold; }
.contract.requires { color: #f0a500; }
.contract.ensures { color: #4ecdc4; }
.contract.effects { color: #c084fc; }
.contract.body { color: #a0d2db; }
.kw { color: #e94560; }
.ty { color: #4ecdc4; }
.op { color: #f0a500; }
.num { color: #a0d2db; }
.str { color: #98c379; }
.cm { color: #666; font-style: italic; }
#search { width: 100%; padding: 8px; margin: 10px 0; background: #16213e; color: #e0e0e0; border: 1px solid #e94560; border-radius: 4px; box-sizing: border-box; }
</style>
</head>
<body>
<div class="container">
<h1>&#x1F5E1; Mumei Documentation</h1>
<p>Auto-generated from source comments.</p>
<input type="text" id="search" placeholder="Search atoms, types…">
<h2>Modules</h2>
<ul class="module-list">
"#,
    );

    for doc in docs {
        index.push_str(&format!(
            "<li><a href=\"{}.html\">{}</a> — {}</li>\n",
            html_escape(&doc.name),
            html_escape(&doc.name),
            html_escape(&doc.file_path)
        ));
    }
    index.push_str(
        r#"</ul>
<script>
// Plan 10 (Task 4F): client-side search filter for the module index.
document.getElementById('search').addEventListener('input', function (e) {
    var q = e.target.value.toLowerCase();
    document.querySelectorAll('.module-list li').forEach(function (li) {
        li.style.display = li.textContent.toLowerCase().includes(q) ? '' : 'none';
    });
});
</script>
</div>
</body>
</html>
"#,
    );
    let _ = fs::write(out_dir.join("index.html"), &index);

    // 各モジュールのページ
    for doc in docs {
        let mut page = format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{name} — Mumei Documentation</title>
<style>
body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif; margin: 0; padding: 20px; background: #1a1a2e; color: #e0e0e0; }}
.container {{ max-width: 900px; margin: 0 auto; }}
h1 {{ color: #e94560; border-bottom: 2px solid #e94560; padding-bottom: 10px; }}
h2 {{ color: #e94560; background: #16213e; padding: 8px 16px; border-radius: 4px; }}
a {{ color: #e94560; text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
.atom {{ background: #16213e; padding: 12px 16px; margin: 8px 0; border-radius: 6px; border-left: 3px solid #e94560; }}
.atom .name {{ color: #e94560; font-weight: bold; font-size: 1.1em; }}
.atom .params {{ color: #a0a0a0; font-family: monospace; }}
.atom .comment {{ color: #c0c0c0; margin-top: 6px; }}
.badge {{ display: inline-block; padding: 2px 8px; border-radius: 3px; font-size: 0.8em; margin-left: 8px; }}
.badge.trusted {{ background: #0f3460; color: #e94560; }}
.badge.verified {{ background: #1a4040; color: #4ecdc4; }}
.badge.async {{ background: #3d1a60; color: #c084fc; }}
.type-def {{ background: #16213e; padding: 8px 16px; margin: 8px 0; border-radius: 6px; border-left: 3px solid #4ecdc4; }}
.contract {{ font-family: monospace; padding: 4px 16px; margin: 2px 0; font-size: 0.9em; }}
.contract .label {{ font-weight: bold; }}
.contract.requires {{ color: #f0a500; }}
.contract.ensures {{ color: #4ecdc4; }}
.contract.effects {{ color: #c084fc; }}
.contract.body {{ color: #a0d2db; }}
.kw {{ color: #e94560; }}
.ty {{ color: #4ecdc4; }}
.op {{ color: #f0a500; }}
.num {{ color: #a0d2db; }}
.str {{ color: #98c379; }}
.cm {{ color: #666; font-style: italic; }}
</style>
</head>
<body>
<div class="container">
<p><a href="index.html">&larr; Back to index</a></p>
<h1>{name}</h1>
<p>Source: <code>{path}</code></p>
"#,
            name = html_escape(&doc.name),
            path = html_escape(&doc.file_path),
        );

        // Atoms
        if !doc.atoms.is_empty() {
            page.push_str("<h2>Atoms</h2>\n");
            for atom in &doc.atoms {
                page.push_str("<div class=\"atom\">\n");
                page.push_str(&format!(
                    "  <span class=\"name\">{}</span>",
                    html_escape(&atom.name)
                ));
                if atom.is_async {
                    page.push_str("<span class=\"badge async\">async</span>");
                }
                if atom.trust_level == "Trusted" {
                    page.push_str("<span class=\"badge trusted\">trusted</span>");
                } else if atom.trust_level == "Verified" {
                    page.push_str("<span class=\"badge verified\">verified</span>");
                }
                let escaped_params: Vec<String> =
                    atom.params.iter().map(|p| html_escape(p)).collect();
                page.push_str(&format!(
                    "\n  <div class=\"params\">({})</div>\n",
                    escaped_params.join(", ")
                ));
                if !atom.comment.is_empty() {
                    page.push_str(&format!(
                        "  <div class=\"comment\">{}</div>\n",
                        html_escape(&atom.comment).replace('\n', "<br>")
                    ));
                }
                // Plan 10 (Task 4C): inline contract metadata (requires /
                // ensures / effects / body) below each atom. Skip trivial
                // `requires: true` / `ensures: true` clauses to keep the
                // page noise-free.
                if !atom.requires.is_empty() && atom.requires != "true" {
                    page.push_str(&format!(
                        "  <div class=\"contract requires\"><span class=\"label\">requires:</span> <code>{}</code></div>\n",
                        highlight_mumei_code(&atom.requires)
                    ));
                }
                if !atom.ensures.is_empty() && atom.ensures != "true" {
                    page.push_str(&format!(
                        "  <div class=\"contract ensures\"><span class=\"label\">ensures:</span> <code>{}</code></div>\n",
                        highlight_mumei_code(&atom.ensures)
                    ));
                }
                if !atom.effects.is_empty() {
                    let effects_html: Vec<String> =
                        atom.effects.iter().map(|e| html_escape(e)).collect();
                    page.push_str(&format!(
                        "  <div class=\"contract effects\"><span class=\"label\">effects:</span> <code>[{}]</code></div>\n",
                        effects_html.join(", ")
                    ));
                }
                if !atom.body_expr.is_empty() {
                    page.push_str(&format!(
                        "  <div class=\"contract body\"><span class=\"label\">body:</span> <code>{}</code></div>\n",
                        highlight_mumei_code(&atom.body_expr)
                    ));
                }
                page.push_str("</div>\n");
            }
        }

        // Types
        if !doc.types.is_empty() {
            page.push_str("<h2>Types</h2>\n");
            for t in &doc.types {
                page.push_str(&format!(
                    "<div class=\"type-def\"><strong>{}</strong>",
                    html_escape(&t.name)
                ));
                if !t.comment.is_empty() {
                    page.push_str(&format!(
                        "<br>{}",
                        html_escape(&t.comment).replace('\n', "<br>")
                    ));
                }
                page.push_str("</div>\n");
            }
        }

        // Structs
        if !doc.structs.is_empty() {
            page.push_str("<h2>Structs</h2>\n");
            for s in &doc.structs {
                page.push_str(&format!(
                    "<div class=\"type-def\"><strong>struct {}</strong>",
                    html_escape(&s.name)
                ));
                if !s.comment.is_empty() {
                    page.push_str(&format!(
                        "<br>{}",
                        html_escape(&s.comment).replace('\n', "<br>")
                    ));
                }
                page.push_str("</div>\n");
            }
        }

        // Enums
        if !doc.enums.is_empty() {
            page.push_str("<h2>Enums</h2>\n");
            for e in &doc.enums {
                page.push_str(&format!(
                    "<div class=\"type-def\"><strong>enum {}</strong>",
                    html_escape(&e.name)
                ));
                if !e.comment.is_empty() {
                    page.push_str(&format!(
                        "<br>{}",
                        html_escape(&e.comment).replace('\n', "<br>")
                    ));
                }
                page.push_str("</div>\n");
            }
        }

        // Traits
        if !doc.traits.is_empty() {
            page.push_str("<h2>Traits</h2>\n");
            for t in &doc.traits {
                page.push_str(&format!(
                    "<div class=\"type-def\"><strong>trait {}</strong>",
                    html_escape(&t.name)
                ));
                if !t.comment.is_empty() {
                    page.push_str(&format!(
                        "<br>{}",
                        html_escape(&t.comment).replace('\n', "<br>")
                    ));
                }
                page.push_str("</div>\n");
            }
        }

        page.push_str("</div>\n</body>\n</html>\n");
        let _ = fs::write(out_dir.join(format!("{}.html", doc.name)), &page);
    }
}

/// Markdown ドキュメントを生成
pub(crate) fn generate_markdown_docs(docs: &[ModuleDoc], out_dir: &Path) {
    for doc in docs {
        let mut md = format!("# {}\n\nSource: `{}`\n\n", doc.name, doc.file_path);

        if !doc.atoms.is_empty() {
            md.push_str("## Atoms\n\n");
            for atom in &doc.atoms {
                let badges = [
                    if atom.is_async { Some("`async`") } else { None },
                    if atom.trust_level == "Trusted" {
                        Some("`trusted`")
                    } else {
                        None
                    },
                ]
                .iter()
                .flatten()
                .copied()
                .collect::<Vec<_>>()
                .join(" ");

                md.push_str(&format!(
                    "### `{}({})` {}\n\n",
                    atom.name,
                    atom.params.join(", "),
                    badges
                ));
                if !atom.comment.is_empty() {
                    md.push_str(&format!("{}\n\n", atom.comment));
                }
                // Plan 10 (Task 4C): emit contract metadata as fenced
                // blocks under each atom. Mirrors the HTML output so that
                // markdown consumers (GitHub Pages, mdbook, etc.) see the
                // same information.
                if !atom.requires.is_empty() && atom.requires != "true" {
                    md.push_str(&format!("- **requires**: ```{}```\n", atom.requires));
                }
                if !atom.ensures.is_empty() && atom.ensures != "true" {
                    md.push_str(&format!("- **ensures**: ```{}```\n", atom.ensures));
                }
                if !atom.effects.is_empty() {
                    md.push_str(&format!("- **effects**: `[{}]`\n", atom.effects.join(", ")));
                }
                if !atom.body_expr.is_empty() {
                    md.push_str(&format!("- **body**: ```{}```\n", atom.body_expr));
                }
                if !atom.requires.is_empty()
                    || !atom.ensures.is_empty()
                    || !atom.effects.is_empty()
                    || !atom.body_expr.is_empty()
                {
                    md.push('\n');
                }
            }
        }

        if !doc.types.is_empty() {
            md.push_str("## Types\n\n");
            for t in &doc.types {
                md.push_str(&format!("### `{}`\n\n", t.name));
                if !t.comment.is_empty() {
                    md.push_str(&format!("{}\n\n", t.comment));
                }
            }
        }

        if !doc.structs.is_empty() {
            md.push_str("## Structs\n\n");
            for s in &doc.structs {
                md.push_str(&format!("### `struct {}`\n\n", s.name));
                if !s.comment.is_empty() {
                    md.push_str(&format!("{}\n\n", s.comment));
                }
            }
        }

        if !doc.enums.is_empty() {
            md.push_str("## Enums\n\n");
            for e in &doc.enums {
                md.push_str(&format!("### `enum {}`\n\n", e.name));
                if !e.comment.is_empty() {
                    md.push_str(&format!("{}\n\n", e.comment));
                }
            }
        }

        if !doc.traits.is_empty() {
            md.push_str("## Traits\n\n");
            for t in &doc.traits {
                md.push_str(&format!("### `trait {}`\n\n", t.name));
                if !t.comment.is_empty() {
                    md.push_str(&format!("{}\n\n", t.comment));
                }
            }
        }

        let _ = fs::write(out_dir.join(format!("{}.md", doc.name)), &md);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use crate::commands::repl::{repl_wrap_expr, REPL_EVAL_ATOM};
    use crate::feedback::{parse_artifact_paths, parse_intent_fidelity_json};
    use clap::Parser;
    use mumei_core::verification;

    // --- P3-B: Doc Gen テスト ---

    /// extract_comment_before: コメントが直前にある場合
    #[test]
    fn test_extract_comment_before_single_line() {
        let lines = vec!["// This is a doc comment", "atom foo() -> i64"];
        let comment = extract_comment_before(&lines, 2); // target_line is 1-indexed
        assert_eq!(comment, "This is a doc comment");
    }

    /// extract_comment_before: 複数行コメント
    #[test]
    fn test_extract_comment_before_multi_line() {
        let lines = vec![
            "// First line",
            "// Second line",
            "// Third line",
            "atom bar() -> i64",
        ];
        let comment = extract_comment_before(&lines, 4);
        assert_eq!(comment, "First line\nSecond line\nThird line");
    }

    /// extract_comment_before: コメントがない場合
    #[test]
    fn test_extract_comment_before_no_comment() {
        let lines = vec!["let x = 42;", "atom baz() -> i64"];
        let comment = extract_comment_before(&lines, 2);
        assert_eq!(comment, "");
    }

    /// extract_comment_before: 境界値テスト (行0)
    #[test]
    fn test_extract_comment_before_line_zero() {
        let lines = vec!["// comment", "atom foo() -> i64"];
        let comment = extract_comment_before(&lines, 0);
        assert_eq!(comment, "");
    }

    /// extract_comment_before: 範囲外の行
    #[test]
    fn test_extract_comment_before_out_of_range() {
        let lines = vec!["// comment"];
        let comment = extract_comment_before(&lines, 100);
        assert_eq!(comment, "");
    }

    /// extract_comment_before: コメントの間に空行がある場合（途切れる）
    #[test]
    fn test_extract_comment_before_gap() {
        let lines = vec![
            "// Unrelated comment",
            "",
            "// Relevant comment",
            "atom qux() -> i64",
        ];
        let comment = extract_comment_before(&lines, 4);
        assert_eq!(comment, "Relevant comment");
    }

    // --- P3-B: Doc Gen パースからドキュメント抽出テスト ---

    /// P3-B: ModuleDoc / ItemDoc 構造体の構築テスト
    #[test]
    fn test_doc_gen_module_doc_construction() {
        let source = r#"
atom http_get(url: String) -> String
  requires: true;
  ensures: true;
  body: url;

atom json_parse(input: String) -> String
  requires: true;
  ensures: true;
  body: input;
"#;
        let items = parser::parse_module(source);

        let mut doc = ModuleDoc {
            name: "test_module".to_string(),
            file_path: "test.mm".to_string(),
            atoms: Vec::new(),
            types: Vec::new(),
            structs: Vec::new(),
            enums: Vec::new(),
            traits: Vec::new(),
        };

        for item in &items {
            if let parser::Item::Atom(atom) = item {
                doc.atoms.push(ItemDoc {
                    name: atom.name.clone(),
                    comment: String::new(),
                    params: atom.params.iter().map(|p| p.name.clone()).collect(),
                    trust_level: format!("{:?}", atom.trust_level),
                    is_async: atom.is_async,
                    requires: atom.requires.clone(),
                    ensures: atom.ensures.clone(),
                    body_expr: atom.body_expr.clone(),
                    effects: atom.effects.iter().map(|e| e.name.clone()).collect(),
                });
            }
        }

        assert_eq!(doc.atoms.len(), 2);
        assert_eq!(doc.atoms[0].name, "http_get");
        assert_eq!(doc.atoms[0].params, vec!["url"]);
        assert_eq!(doc.atoms[0].trust_level, "Verified");
        assert!(!doc.atoms[0].is_async);
        assert_eq!(doc.atoms[1].name, "json_parse");
        assert_eq!(doc.atoms[1].params, vec!["input"]);
    }

    #[test]
    fn test_parse_artifact_paths_trims_and_ignores_empty_entries() {
        assert_eq!(
            parse_artifact_paths(" reports/a.json, ,out/b.json "),
            Some(vec!["reports/a.json".to_string(), "out/b.json".to_string()])
        );
        assert!(parse_artifact_paths(" , ").is_none());
    }

    #[test]
    fn test_parse_intent_fidelity_json() {
        let metadata = parse_intent_fidelity_json(
            r#"{"natural_language_prompt_hash":"sha256:prompt","spec_traceability_score":0.97,"semantic_drift_detected":false,"manual_review_required":true}"#,
        )
        .unwrap();

        assert_eq!(
            metadata.natural_language_prompt_hash.as_deref(),
            Some("sha256:prompt")
        );
        assert_eq!(metadata.spec_traceability_score, 0.97);
        assert!(!metadata.semantic_drift_detected);
        assert!(metadata.manual_review_required);
        assert!(parse_intent_fidelity_json("not json").is_err());
    }

    #[test]
    fn test_verify_cli_accepts_harness_metadata_options() {
        let cli = Cli::try_parse_from([
            "mumei",
            "verify",
            "--proof-cert",
            "--harness-contract",
            "contracts/harness.json",
            "--intent-fidelity",
            r#"{"natural_language_prompt_hash":"sha256:prompt"}"#,
            "--artifact-paths",
            "reports/a.json,out/b.json",
            "--budget-policy-fingerprint",
            "sha256:budget",
            "input.mm",
        ])
        .unwrap();

        match cli.command {
            Some(Command::Verify {
                harness_contract,
                intent_fidelity,
                artifact_paths,
                budget_policy_fingerprint,
                ..
            }) => {
                assert_eq!(harness_contract.as_deref(), Some("contracts/harness.json"));
                assert_eq!(
                    intent_fidelity.as_deref(),
                    Some(r#"{"natural_language_prompt_hash":"sha256:prompt"}"#)
                );
                assert_eq!(artifact_paths.as_deref(), Some("reports/a.json,out/b.json"));
                assert_eq!(budget_policy_fingerprint.as_deref(), Some("sha256:budget"));
            }
            _ => panic!("expected verify command"),
        }
    }

    // --- P3-A: REPL ヘルパーテスト ---

    /// P3-A: REPL の atom ラッピングロジックテスト
    #[test]
    fn test_repl_atom_wrapping() {
        // REPL は入力式を atom でラップして検証する
        let expr = "1 + 2";
        let wrapped = repl_wrap_expr(REPL_EVAL_ATOM, expr, None);
        assert!(wrapped.contains("__repl_eval"));
        assert!(wrapped.contains("1 + 2"));
        assert!(wrapped.contains("requires: true"));

        // ラップされた atom がパース可能であること
        let items = parser::parse_module(&wrapped);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let parser::Item::Atom(a) = i {
                    Some(a)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].name, "__repl_eval");
    }

    #[test]
    fn test_repl_float_atom_wrapping_annotates_return_type() {
        let wrapped = repl_wrap_expr(REPL_EVAL_ATOM, "1.5", Some("f64"));
        let items = parser::parse_module(&wrapped);
        let Some(parser::Item::Atom(atom)) = items.first() else {
            panic!("expected wrapped expression atom");
        };

        assert_eq!(atom.name, "__repl_eval");
        assert_eq!(atom.return_type.as_deref(), Some("f64"));
    }

    /// P3-A: REPL :load コマンドのパースロジックテスト
    #[test]
    fn test_repl_load_command_parsing() {
        let input = ":load std/json.mm";
        assert!(input.starts_with(":load "));
        let file = input.strip_prefix(":load ").unwrap().trim();
        assert_eq!(file, "std/json.mm");
    }

    /// P3-A: REPL コマンド認識テスト
    #[test]
    fn test_repl_command_recognition() {
        // 終了コマンド
        assert!(matches!(":quit", ":quit" | ":q" | ":exit"));
        assert!(matches!(":q", ":quit" | ":q" | ":exit"));
        assert!(matches!(":exit", ":quit" | ":q" | ":exit"));

        // ヘルプコマンド
        assert!(matches!(":help", ":help" | ":h"));
        assert!(matches!(":h", ":help" | ":h"));

        // :load コマンド
        let load_cmd = ":load test.mm";
        assert!(load_cmd.starts_with(":load "));

        // :env コマンド
        let env_cmd = ":env";
        assert_eq!(env_cmd, ":env");
    }

    // --- P1-A: FFI Bridge（main.rs の load_and_prepare）テスト ---

    /// P1-A: ExternBlock が load_and_prepare で trusted atom に変換されるテスト
    #[test]
    fn test_load_and_prepare_registers_extern_atoms() {
        let source = r#"
extern "Rust" {
    fn ffi_sqrt(x: f64) -> f64;
    fn ffi_abs(x: i64) -> i64;
}

atom main() -> i64
  requires: true;
  ensures: result == 42;
  body: 42;
"#;
        let items = parser::parse_module(source);
        let mut module_env = verification::ModuleEnv::new();
        verification::register_builtin_traits(&mut module_env);

        // ExternBlock → trusted atom 変換ロジック（load_and_prepare と同等）
        for item in &items {
            match item {
                parser::Item::Atom(atom) => {
                    module_env.register_atom(atom);
                }
                parser::Item::ExternBlock(eb) => {
                    for ext_fn in &eb.functions {
                        let params: Vec<parser::Param> = ext_fn
                            .param_types
                            .iter()
                            .enumerate()
                            .map(|(i, ty)| parser::Param {
                                name: ext_fn
                                    .param_names
                                    .get(i)
                                    .cloned()
                                    .unwrap_or_else(|| format!("arg{}", i)),
                                type_name: Some(ty.clone()),
                                type_ref: Some(parser::parse_type_ref(ty)),
                                is_ref: false,
                                is_ref_mut: false,
                                fn_contract_requires: None,
                                fn_contract_ensures: None,
                            })
                            .collect();
                        let atom = parser::Atom {
                            name: ext_fn.name.clone(),
                            type_params: vec![],
                            where_bounds: vec![],
                            params,
                            trace_id: None,
                            spec_metadata: std::collections::HashMap::new(),
                            requires: "true".to_string(),
                            forall_constraints: vec![],
                            ensures: "true".to_string(),
                            body_expr: String::new(),
                            consumed_params: vec![],
                            resources: vec![],
                            is_async: false,
                            trust_level: parser::TrustLevel::Trusted,
                            max_unroll: None,
                            invariant: None,
                            effects: vec![],
                            return_type: Some(ext_fn.return_type.clone()),
                            span: ext_fn.span.clone(),
                            effect_pre: std::collections::HashMap::new(),
                            effect_post: std::collections::HashMap::new(),
                        };
                        module_env.register_atom(&atom);
                    }
                }
                _ => {}
            }
        }

        // extern 関数が trusted atom として登録されていること
        assert!(module_env.get_atom("ffi_sqrt").is_some());
        assert_eq!(
            module_env.get_atom("ffi_sqrt").unwrap().trust_level,
            parser::TrustLevel::Trusted
        );
        assert!(module_env.get_atom("ffi_abs").is_some());

        // 通常 atom も登録されていること
        assert!(module_env.get_atom("main").is_some());
        assert_eq!(
            module_env.get_atom("main").unwrap().trust_level,
            parser::TrustLevel::Verified
        );
    }

    // --- Phase B: call_with_contract E2E Z3 verification tests ---

    /// Helper: parse source, register items, and verify a single atom by name
    fn verify_atom_from_source(
        source: &str,
        atom_name: &str,
    ) -> Result<(), verification::MumeiError> {
        let items = parser::parse_module(source);
        let mut module_env = verification::ModuleEnv::new();
        verification::register_builtin_traits(&mut module_env);
        for item in &items {
            if let parser::Item::Atom(atom) = item {
                module_env.register_atom(atom);
            }
        }
        let atom = module_env
            .get_atom(atom_name)
            .unwrap_or_else(|| panic!("atom '{}' not found", atom_name))
            .clone();
        let hir_atom = lower_atom_to_hir(&atom);
        verification::verify(&hir_atom, std::path::Path::new("."), &module_env)
    }

    #[test]
    fn test_call_with_contract_basic_ensures() {
        let source = r#"
atom increment(x: i64)
    requires: x >= 0;
    ensures: result == x + 1;
    body: x + 1;

atom apply(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, x);

atom test_apply()
    requires: true;
    ensures: result >= 0;
    body: apply(5, atom_ref(increment));
"#;
        // apply should verify: contract(f) ensures result >= 0 constrains the dynamic call
        assert!(
            verify_atom_from_source(source, "apply").is_ok(),
            "apply with contract(f) ensures should verify"
        );
        // test_apply calls apply with concrete atom_ref
        assert!(
            verify_atom_from_source(source, "test_apply").is_ok(),
            "test_apply should verify via compositional verification"
        );
    }

    #[test]
    fn test_call_with_contract_requires_and_ensures() {
        let source = r#"
atom apply_twice(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    contract(f): requires: x >= 0, ensures: result >= 0;
    body: {
        let first = call(f, x);
        call(f, first)
    }
"#;
        // apply_twice should verify: first call's result >= 0 satisfies second call's requires
        assert!(
            verify_atom_from_source(source, "apply_twice").is_ok(),
            "apply_twice with contract(f) requires+ensures should verify"
        );
    }

    #[test]
    fn test_call_with_contract_binary_function() {
        let source = r#"
atom add(a: i64, b: i64)
    requires: a >= 0 && b >= 0;
    ensures: result == a + b;
    body: a + b;

atom fold_two(a: i64, b: i64, f: atom_ref(i64, i64) -> i64)
    requires: a >= 0 && b >= 0;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, a, b);

atom test_fold()
    requires: true;
    ensures: result >= 0;
    body: fold_two(3, 4, atom_ref(add));
"#;
        assert!(
            verify_atom_from_source(source, "fold_two").is_ok(),
            "fold_two with binary contract should verify"
        );
        assert!(
            verify_atom_from_source(source, "test_fold").is_ok(),
            "test_fold with concrete atom_ref(add) should verify"
        );
    }

    #[test]
    fn test_call_with_contract_in_match() {
        let source = r#"
atom option_map(opt: i64, f: atom_ref(i64) -> i64)
    requires: opt >= 0 && opt <= 1;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: {
        match opt {
            0 => 0,
            _ => call(f, opt)
        }
    }
"#;
        assert!(
            verify_atom_from_source(source, "option_map").is_ok(),
            "option_map with contract in match arm should verify"
        );
    }

    #[test]
    fn test_call_with_contract_without_contract_fails() {
        // An atom using call(f, x) WITHOUT a contract(f) clause
        // should fail verification because the result is unconstrained
        let source = r#"
atom apply_no_contract(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: call(f, x);
"#;
        let result = verify_atom_from_source(source, "apply_no_contract");
        assert!(
            result.is_err(),
            "apply without contract(f) should fail: result is unconstrained"
        );
    }

    // --- Subsumption check tests ---

    #[test]
    fn test_subsumption_check_no_warning_when_implies() {
        // Integration test: increment ensures: result == x + 1, with x >= 0
        // this implies result >= 0. Subsumption holds, so no warning is emitted.
        // The full verification pipeline should succeed for both atoms.
        //
        // NOTE: The subsumption check return value is tested directly in
        // mumei-core/src/verification.rs::tests::test_subsumption_check_holds_with_requires.
        let source = r#"
atom increment(x: i64)
    requires: x >= 0;
    ensures: result == x + 1;
    body: x + 1;

atom apply(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, x);

atom test_apply()
    requires: true;
    ensures: result >= 0;
    body: apply(5, atom_ref(increment));
"#;
        // Both apply and test_apply should verify successfully
        assert!(
            verify_atom_from_source(source, "apply").is_ok(),
            "apply should verify with contract(f)"
        );
        assert!(
            verify_atom_from_source(source, "test_apply").is_ok(),
            "test_apply with atom_ref(increment) should verify (subsumption holds)"
        );
    }

    #[test]
    fn test_subsumption_check_warning_when_not_implies() {
        // Integration test: negate ensures: result == 0 - x, which does NOT
        // imply result >= 0 even under requires: x >= 0.
        // The subsumption check emits a warning to stderr, but verification
        // of the caller still passes because the contract is trusted
        // (warning only, not a hard error).
        //
        // NOTE: The subsumption check return value (false = warning emitted)
        // is tested directly in
        // mumei-core/src/verification.rs::tests::test_subsumption_check_fails_without_requires.
        let source = r#"
atom negate(x: i64)
    requires: x >= 0;
    ensures: result == 0 - x;
    body: 0 - x;

atom apply(x: i64, f: atom_ref(i64) -> i64)
    requires: x >= 0;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, x);

atom test_apply_negate()
    requires: true;
    ensures: result >= 0;
    body: apply(5, atom_ref(negate));
"#;
        // test_apply_negate should still verify (subsumption is a warning, not error)
        // The compositional verification trusts the contract's ensures.
        let result = verify_atom_from_source(source, "test_apply_negate");
        assert!(
            result.is_ok(),
            "test_apply_negate should verify (subsumption warning only, not error): {:?}",
            result.err()
        );
    }

    #[test]
    fn test_subsumption_existing_tests_still_pass() {
        // Regression guard: Ensure the basic call_with_contract_basic_ensures
        // scenario still works after subsumption check integration.
        let source = r#"
atom add(a: i64, b: i64)
    requires: a >= 0 && b >= 0;
    ensures: result == a + b;
    body: a + b;

atom fold_two(a: i64, b: i64, f: atom_ref(i64, i64) -> i64)
    requires: a >= 0 && b >= 0;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, a, b);

atom test_fold()
    requires: true;
    ensures: result >= 0;
    body: fold_two(3, 4, atom_ref(add));
"#;
        assert!(
            verify_atom_from_source(source, "fold_two").is_ok(),
            "fold_two should still verify after subsumption integration"
        );
        assert!(
            verify_atom_from_source(source, "test_fold").is_ok(),
            "test_fold with atom_ref(add) should still verify"
        );
    }

    // --- Feature 1: Semantic Feedback Tests ---

    /// Test constraint_to_natural_language for various patterns
    #[test]
    fn test_constraint_to_natural_language() {
        // v <= N pattern
        let result =
            verification::constraint_to_natural_language("age", "HumanAge", "v <= 120", "121");
        assert!(result.contains("age"), "should mention param name");
        assert!(result.contains("121"), "should mention actual value");
        assert!(result.contains("120"), "should mention bound");
        assert!(result.contains("HumanAge"), "should mention type name");
        // Bilingual: should contain both English and Japanese
        assert!(
            result.contains("violates") || result.contains("exceeds"),
            "should have English text"
        );

        // v >= N pattern
        let result2 = verification::constraint_to_natural_language("x", "Nat", "v >= 0", "-1");
        assert!(result2.contains("x"), "should mention param name");
        assert!(result2.contains("-1"), "should mention actual value");

        // Modulo pattern (now matched by try_match_modulo)
        let result3 =
            verification::constraint_to_natural_language("val", "Custom", "v % 2 == 0", "3");
        assert!(result3.contains("val"), "should mention param name");
        assert!(
            result3.contains("multiple of") || result3.contains("倍数"),
            "should describe modulo constraint"
        );
    }

    /// Test suggestion_for_failure_type returns appropriate suggestions
    #[test]
    fn test_suggestion_for_failure_type() {
        let s1 =
            verification::suggestion_for_failure_type(verification::FAILURE_POSTCONDITION_VIOLATED);
        assert!(
            !s1.is_empty(),
            "postcondition suggestion should not be empty"
        );

        let s2 = verification::suggestion_for_failure_type(verification::FAILURE_DIVISION_BY_ZERO);
        assert!(
            s2.contains("0") || s2.contains("zero") || s2.contains("divisor"),
            "division by zero suggestion should mention zero or divisor"
        );

        let s3 = verification::suggestion_for_failure_type("unknown_type");
        assert!(
            !s3.is_empty(),
            "unknown failure type should still return a suggestion"
        );
    }

    /// Test build_semantic_feedback produces valid JSON structure
    #[test]
    fn test_build_semantic_feedback() {
        let source = r#"
type HumanAge = i64 where v >= 0 && v <= 120;

atom validate_age(age: HumanAge) -> i64
  requires: true;
  ensures: result >= 0 && result <= 120;
  body: age;
"#;
        let items = parser::parse_module(source);
        let mut module_env = verification::ModuleEnv::new();

        // Register type
        for item in &items {
            if let parser::Item::TypeDef(td) = item {
                module_env.register_type(td);
            }
        }

        // Find atom and build feedback
        for item in &items {
            if let parser::Item::Atom(atom) = item {
                let mappings = verification::build_constraint_mappings_for_atom(atom, &module_env);
                assert!(
                    !mappings.is_empty(),
                    "should have constraint mappings for HumanAge param"
                );

                let counterexample = serde_json::json!({"age": "121"});
                let feedback = verification::build_semantic_feedback(
                    &mappings,
                    Some(&counterexample),
                    atom,
                    verification::FAILURE_POSTCONDITION_VIOLATED,
                    None,
                );
                assert!(feedback.is_some(), "should produce semantic feedback");
                let feedback = feedback.unwrap();

                // feedback should have violated_constraints
                let vc = feedback.get("violated_constraints");
                assert!(vc.is_some(), "should have violated_constraints field");
                let vc_arr = vc.unwrap().as_array().unwrap();
                assert!(
                    !vc_arr.is_empty(),
                    "should have at least one violated constraint for HumanAge"
                );

                let first = &vc_arr[0];
                assert_eq!(first["param"], "age");
                assert_eq!(first["type"], "HumanAge");
                assert!(
                    !first["explanation"].as_str().unwrap().is_empty(),
                    "explanation should not be empty"
                );
                assert!(
                    !first["suggestion"].as_str().unwrap().is_empty(),
                    "suggestion should not be empty"
                );

                // Should have context section
                let ctx = feedback.get("context");
                assert!(ctx.is_some(), "should have context field");
            }
        }
    }
}
