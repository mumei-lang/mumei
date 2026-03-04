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
| Rich Diagnostics (miette/ariadne) | ❌ 未実装 (将来) |

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

### 現在のエラー型（実装済み）

```rust
pub enum MumeiError {
    VerificationError { msg: String, span: Span },
    CodegenError { msg: String, span: Span },
    TypeError { msg: String, span: Span },
}

pub struct ErrorDetail {
    pub message: String,
    pub span: Span,
    pub suggestion: Option<String>,
}
```

コンストラクタ:
- `MumeiError::verification(msg)` — Span なし（位置不明）
- `MumeiError::verification_at(msg, span)` — Span 付き
- `MumeiError::type_error_at(msg, span)` — 型エラー + Span

### LSP での活用（実装済み）

`lsp.rs` の `verify_source_for_lsp` が Z3 検証エラーを `ErrorDetail` に変換し、
`publishDiagnostics` で正確な行・列を送信:

```
textDocument/publishDiagnostics:
  - range: ErrorDetail.span から 0-indexed に変換
  - message: ErrorDetail の Display 出力
  - severity: 1 (Error)
```

### 将来の Rich Diagnostics

段階的に以下の機能を導入予定:

1. **miette / ariadne 統合**: カラー表示・下線・サジェスト付きターミナル出力
2. **suggestion フィールド活用**: `ErrorDetail.suggestion` による修正提案の表示
3. **マルチスパン**: 1つのエラーに対して複数の関連位置を表示

## 設計原則

1. **全ノードに位置情報**: パーサーが生成する全 AST ノードに Span を付与
2. **伝播**: monomorphize (ジェネリクス展開) 時にも Span を保持・伝播
3. **段階的導入**: Span → MumeiError 統合 → ErrorDetail → Rich Diagnostics
4. **後方互換**: 既存のエラーメッセージは維持しつつ、段階的に改善

## 関連ファイル

- `src/parser.rs` — `Span` 構造体定義、`offset_to_line_col`、`span_from_offset` ヘルパー
- `src/verification.rs` — `MumeiError`、`ErrorDetail`、Span 付きエラー生成
- `src/lsp.rs` — `find_error_position`、`verify_source_for_lsp`
