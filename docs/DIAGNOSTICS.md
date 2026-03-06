# Diagnostics Design Document

> Mumei の診断 (Diagnostics) 基盤設計

## 概要

Mumei コンパイラは、開発者に対して正確で有用なエラーメッセージを提供するために、
ソースコード上の位置情報 (Span) を全 AST ノードに付与する診断駆動設計を採用しています。

## 実装状況

| 項目 | ステータス |
|---|---|
| `Span` 構造体 | ✅ 実装済み (`src/parser.rs`) |
| 全 AST Item 型に `span` フィールド | ✅ 実装済み (Atom, StructDef, EnumDef, TraitDef, ImplDef, ResourceDef, ImportDecl, RefinedType, ExternBlock, ExternFn) |
| `MumeiError` に Span 統合 | ✅ 実装済み (`VerificationError { msg, span }` 等) |
| `ErrorDetail` 構造体 | ✅ 実装済み (message + span + suggestion) |
| `offset_to_line_col` ヘルパー | ✅ 実装済み (regex マッチから行・列を計算) |
| LSP diagnostics に Span 反映 | ✅ 実装済み (`lsp.rs` の `find_error_position`) |
| Rich Diagnostics (miette) | ✅ 実装済み (`miette` crate によるカラー出力・ソースハイライト・サジェスト) |

## Span 情報

### 構造

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub len: usize,
}
```

| フィールド | 説明 |
|---|---|
| `file` | ソースファイルパス（空文字列は不明を表す） |
| `line` | 行番号 (1-indexed、0 は不明) |
| `col` | 列番号 (1-indexed、0 は不明) |
| `len` | トークンの文字数（0 は不明） |

### 適用範囲

Span は以下の全 AST 型に付与済み:

- `Atom` — 関数定義
- `StructDef`, `EnumDef` — 型定義
- `TraitDef`, `ImplDef` — トレイト/実装定義
- `ResourceDef` — リソース定義
- `ImportDecl` — インポート宣言
- `RefinedType` — 精緻型定義
- `ExternBlock`, `ExternFn` — FFI 外部関数宣言

## エラー報告戦略

### エラー型（miette::Diagnostic 対応）

```rust
#[derive(thiserror::Error, miette::Diagnostic, Debug)]
pub enum MumeiError {
    #[error("Verification Error: {msg}")]
    #[diagnostic(code(mumei::verification))]
    VerificationError {
        msg: String,
        #[source_code]
        src: miette::NamedSource<String>,
        #[label("verification failed here")]
        span: miette::SourceSpan,
        #[help]
        help: Option<String>,
    },
    // CodegenError, TypeError も同様の構造
}
```

コンストラクタ:
- `MumeiError::verification(msg)` — Span なし（位置不明）
- `MumeiError::verification_at(msg, span)` — Span 付き
- `MumeiError::verification_with_source(msg, span, source, help)` — ソースコード付きリッチ出力
- `MumeiError::type_error_at(msg, span)` — 型エラー + Span
- `.with_source(source, span)` — 既存エラーにソースコードを後付け
- `.with_help(msg)` — ヘルプメッセージを後付け

ヘルパー関数:
- `span_to_source_span(source, span)` — `Span`(line/col/len) から `miette::SourceSpan`(バイトオフセット) に変換

### LSP での活用（実装済み）

`lsp.rs` の `verify_source_for_lsp` が Z3 検証エラーを `ErrorDetail` に変換し、
`publishDiagnostics` で正確な行・列を送信:

```
textDocument/publishDiagnostics:
  - range: ErrorDetail.span から 0-indexed に変換
  - message: ErrorDetail の Display 出力
  - severity: 1 (Error)
```

### Rich Diagnostics 出力例 (miette)

miette によるリッチなエラー出力の例:

```
  × Verification Error: Postcondition (ensures) is not satisfied.
   ╭─[examples/basic.mm:5:1]
 4 │   ensures: result > 0;
 5 │   body: x - 1;
   ·   ──────────── verification failed here
 6 │
   ╰────
  help: ensures の条件を確認してください。body の返り値が事後条件を満たすか検討してください
```

```
  × Verification Error: Potential division by zero.
  help: requires に除数 != 0 の条件を追加してください
```

```
  × Verification Error: Call to 'foo': precondition (requires) not satisfied at call site
  help: 呼び出し元で事前条件を満たしていません。引数の制約を確認してください
```

### 将来の拡張

1. **マルチスパン**: 1つのエラーに対して複数の関連位置を表示
2. **スナップショットテスト**: miette 出力の回帰テスト
3. **LSP 統合強化**: miette の構造化情報を LSP diagnostics にも活用

## 設計原則

1. **全ノードに位置情報**: パーサーが生成する全 AST ノードに Span を付与
2. **伝播**: monomorphize (ジェネリクス展開) 時にも Span を保持・伝播
3. **段階的導入**: Span → MumeiError 統合 → ErrorDetail → Rich Diagnostics (miette)
4. **後方互換**: 既存のエラーメッセージは維持しつつ、段階的に改善
5. **サジェスト駆動**: エラーに対して具体的な修正提案を `#[help]` で表示

## 関連ファイル

- `src/parser.rs` — `Span` 構造体定義、`offset_to_line_col`、`span_from_offset` ヘルパー
- `src/verification.rs` — `MumeiError`、`ErrorDetail`、Span 付きエラー生成
- `src/lsp.rs` — `find_error_position`、`verify_source_for_lsp`
