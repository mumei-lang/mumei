# FFI (Foreign Function Interface) Design Document

> Mumei の FFI-first 設計と Bridge メカニズム

## 概要

Mumei は **FFI-first** の設計思想を採用し、Rust や C の既存エコシステムと
安全に連携するための外部関数インターフェースを提供します。
FFI 関数は `trusted atom` として自動登録され、コントラクト (requires/ensures) のみで
検証されます (body は外部実装)。

## extern ブロック構文

### 基本構文

```mumei
extern "Rust" {
    fn sqrt(x: f64) -> f64;
    fn abs(x: i64) -> i64;
}
```

### 構文要素

| 要素 | 説明 |
|---|---|
| `extern` | 外部関数ブロックの開始キーワード |
| `"Rust"` / `"C"` | 対象言語名 |
| `fn name(params) -> RetType;` | 関数シグネチャ宣言 |

### AST 表現（実装済み）

```rust
pub struct ExternFn {
    pub name: String,
    pub param_types: Vec<String>,
    pub return_type: String,
    pub span: Span,             // ソース位置情報
}

pub struct ExternBlock {
    pub language: String,
    pub functions: Vec<ExternFn>,
    pub span: Span,             // ソース位置情報
}
```

`Item::ExternBlock(ExternBlock)` として `parse_module` の結果に含まれる。
`main.rs`, `resolver.rs`, `lsp.rs` の全 match ブロックで処理済み。

## 実装状況

| 項目 | ステータス |
|---|---|
| extern ブロック構文パース | ✅ 実装済み |
| `ExternFn` / `ExternBlock` AST | ✅ 実装済み (Span 付き) |
| `Item::ExternBlock` バリアント | ✅ 実装済み (全 match 網羅) |
| パーサーテスト | ✅ 実装済み (`test_parse_extern_block`, `test_parse_extern_block_c`) |
| trusted atom 自動登録 | ❌ 未実装 (将来: extern → ModuleEnv 自動登録) |
| LLVM コード生成 | ❌ 未実装 (将来: extern 関数の declare + call) |

## Bridge メカニズム（設計）

### trusted atom との連携（将来実装予定）

`extern` ブロックで宣言された関数は、将来的に `trusted atom` として自動登録される予定:

1. **body 検証スキップ**: 外部実装のため、body の Z3 検証は行わない
2. **コントラクト検証**: `requires` / `ensures` のコントラクトは呼び出し側で検証される
3. **Taint 分析**: `trusted` 関数の戻り値には `__tainted_` マーカーが付与される

> NOTE: 現時点では `ExternFn` のフィールドは読み取り側が未実装のため `#[allow(dead_code)]` を付与。
> 将来 trusted atom として ModuleEnv に自動登録する際に使用予定。

### 使用例

```mumei
extern "Rust" {
    fn sqrt(x: f64) -> f64;
}

atom safe_sqrt(x: f64) -> f64
    requires: x >= 0.0;
    ensures: result >= 0.0;
    body: sqrt(x);
```

## 対応言語

| 言語 | ステータス | 説明 |
|---|---|---|
| Rust | 設計済み | Rust クレートの `extern "C"` シンボルを参照 |
| C | 設計済み | C ライブラリの関数シンボルを参照 |

## パース処理

`parse_module()` 内で以下の正規表現により extern ブロックを検出:

```
extern\s+"(\w+)"\s*\{([^}]*)\}
```

各関数シグネチャは以下のパターンで抽出:

```
fn\s+(\w+)\s*\(([^)]*)\)\s*->\s*(\w+)
```

## 将来の拡展

> 詳細: [`docs/ROADMAP.md`](ROADMAP.md) Phase P1-A

### 🥇 Priority 1: FFI Bridge Completion (Roadmap P1-A)

std.http / std.json の前提条件として、FFI Bridge の完成が最優先事項です。

**実装計画**:

1. **ExternBlock → trusted atom 自動変換**
   - `ExternFn` のシグネチャから `Atom` を生成
   - `TrustLevel::Trusted` を設定（body 検証スキップ）
   - `ModuleEnv.atoms` に自動登録

2. **LLVM declare 生成**
   - extern 関数を LLVM IR の `declare` として出力
   - 型マッピング: Mumei 型 → LLVM 型

3. **呼び出し側コード生成**
   - ModuleEnv に登録された extern atom への call 生成
   - ABI 互換性の確保 (extern "C" / extern "Rust")

**変更対象ファイル**:
- `src/main.rs` — `load_and_prepare()` で ExternBlock → atom 変換
- `src/verification.rs` — extern atom の trusted 検証
- `src/codegen.rs` — LLVM `declare` + `call` 生成

### その他の拡張

1. **std.http バックエンド** (Roadmap P1-C): reqwest を FFI で隠蔽した HTTP クライアント
2. **std.json バックエンド** (Roadmap P1-B): serde_json を FFI で隠蔽した JSON 操作
3. **std 階層化**: `std.core` / `std.net` / `std.math` モジュール階層への再編成
4. **リンク指示**: `#[link(name = "libm")]` 相当のリンカ指示構文
5. **型マッピング**: Mumei 型と外部言語型の自動マッピング

## 関連ファイル

- `src/parser.rs` — `ExternFn`, `ExternBlock` 構造体定義 + パース処理
- `src/verification.rs` — `TrustLevel::Trusted` による検証スキップ
- `src/codegen.rs` — LLVM IR での外部関数呼び出し生成 (将来)
