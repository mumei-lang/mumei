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

### AST 表現

```rust
pub struct ExternFn {
    pub name: String,
    pub param_types: Vec<String>,
    pub return_type: String,
}

pub struct ExternBlock {
    pub language: String,
    pub functions: Vec<ExternFn>,
}
```

## Bridge メカニズム

### trusted atom との連携

`extern` ブロックで宣言された関数は、内部的に `trusted atom` として扱われます:

1. **body 検証スキップ**: 外部実装のため、body の Z3 検証は行わない
2. **コントラクト検証**: `requires` / `ensures` のコントラクトは呼び出し側で検証される
3. **Taint 分析**: `trusted` 関数の戻り値には `__tainted_` マーカーが付与される

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

1. **SIMD/HTTP バックエンド**: LLVM IR コード生成で Rust FFI 呼び出しを生成
2. **std 階層化**: `std.core` / `std.net` / `std.math` モジュール階層への再編成
3. **リンク指示**: `#[link(name = "libm")]` 相当のリンカ指示構文
4. **型マッピング**: Mumei 型と外部言語型の自動マッピング

## 関連ファイル

- `src/parser.rs` — `ExternFn`, `ExternBlock` 構造体定義 + パース処理
- `src/verification.rs` — `TrustLevel::Trusted` による検証スキップ
- `src/codegen.rs` — LLVM IR での外部関数呼び出し生成 (将来)
