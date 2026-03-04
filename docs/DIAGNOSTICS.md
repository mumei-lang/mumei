# Diagnostics Design Document

> Mumei の診断 (Diagnostics) 基盤設計

## 概要

Mumei コンパイラは、開発者に対して正確で有用なエラーメッセージを提供するために、
ソースコード上の位置情報 (Span) を全 AST ノードに付与する診断駆動設計を採用しています。

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
| `file` | ソースファイルパス |
| `line` | 行番号 (1-indexed) |
| `col` | 列番号 (1-indexed) |
| `len` | トークンの文字数 |

### 適用範囲

Span は以下の全 AST 型に付与されます:

- `Expr` — 全式ノード (変数参照、二項演算、関数呼び出し等)
- `Atom` — 関数定義
- `Pattern` — パターンマッチの各パターン
- `StructDef`, `EnumDef` — 型定義
- `TraitDef`, `ImplDef` — トレイト/実装定義
- `ResourceDef` — リソース定義

## エラー報告戦略

### 現在のエラー型

```rust
pub enum MumeiError {
    VerificationError(String),
    CodegenError(String),
    TypeError(String),
}
```

### 将来の Rich Diagnostics

段階的に以下の機能を導入予定:

1. **ErrorDetail 構造体**: Span 情報 + サジェスト情報を含む構造化エラー
2. **Rich Diagnostics**: miette / ariadne ライブラリによるカラー表示・下線・サジェスト
3. **LSP 連携**: `publishDiagnostics` が正確な行・列を指すように改善

### LSP での活用

```
textDocument/publishDiagnostics:
  - range: Span から計算した正確な位置
  - message: Z3 検証エラーの詳細メッセージ
  - severity: Error / Warning / Information
```

## 設計原則

1. **全ノードに位置情報**: パーサーが生成する全 AST ノードに Span を付与
2. **伝播**: monomorphize (ジェネリクス展開) 時にも Span を保持・伝播
3. **段階的導入**: まず Span 構造の導入 (PR-A)、次にエラー型の拡張、最後に Rich Diagnostics
4. **後方互換**: 既存のエラーメッセージは維持しつつ、段階的に改善

## 関連 PR

- **PR-A** (Span 情報導入): 全 AST 型に `span: Span` フィールドを追加
- 将来: ErrorDetail 導入、Rich Diagnostics ライブラリ統合
