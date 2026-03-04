# Mumei Development Instructions

> このファイルは AI エージェント・コントリビューター向けの開発方針を記述します。

## 現在のフェーズ: 言語機能の充実

### LSP 対応について

LSP (`mumei lsp`) は以下の機能を実装済みであり、**一旦ここまでで完了**とします:

- `textDocument/didOpen` / `textDocument/didChange` → パースエラー diagnostics
- `textDocument/hover` → atom の requires/ensures 表示
- Z3 検証エラーの diagnostics 送信（Span 付き位置情報）
- `shutdown` / `exit` ハンドリング

**深追いしない項目** (将来の余裕があるときに対応):
- `textDocument/completion` (キーワード・atom 名補完)
- `textDocument/definition` (定義ジャンプ)
- Counter-example のエディタ内ハイライト
- VS Code Marketplace への公開

理由: エディタ対応よりも **言語機能の充実** を優先するフェーズに移行するため。

---

## 次のステップ: 言語のアイデンティティ強化

Mumei の差別化ポイント:
- **Rust の安全性** (Z3 形式検証) を持ちつつ
- **Go のようにスラスラ並行処理が書ける** シンプルさ

### 優先度 1: `std.http` の実装

「Mumei ならネットワーク通信がこんなにシンプルに書ける」というデモを作る。

**設計方針:**
- Rust の `reqwest` クレートを FFI (`extern "Rust"`) で隠蔽
- Mumei らしい綺麗な API インターフェースを設計
- `task` / `task_group` と組み合わせた並行 HTTP リクエストのデモ

**想定 API:**
```mumei
import "std/http";

// シンプルな GET
let response = await http.get("https://api.example.com/users");

// 並行リクエスト
task_group:all {
    task { http.get("https://api.example.com/users") };
    task { http.get("https://api.example.com/orders") };
    task { http.get("https://api.example.com/products") }
}
```

### 優先度 2: Task（並行処理）の洗練

PR-C で導入した `task` / `task_group` の実用性を高める。

**TODO:**
- タスクの戻り値型の推論
- `task_group` の結果を変数にバインドする構文
- タスクキャンセルのセマンティクス設計
- チャネル型 (`chan<T>`) の設計

### 優先度 3: FFI Bridge の完成

`extern "Rust" { fn ...; }` で宣言した関数を `trusted atom` として ModuleEnv に自動登録する。
これにより `std.http` の FFI バックエンドが動作する。

---

## コーディング規約

- `cargo fmt` を適用すること
- 新しい `Expr` バリアントを追加する場合、以下の全箇所に match アームを追加:
  - `src/verification.rs`: `expr_to_z3`, `collect_callees`, `count_self_calls`, `collect_acquire_resources`
  - `src/ast.rs`: `collect_from_expr`
  - `src/codegen.rs`: `compile_expr`
  - `src/transpiler/rust.rs`, `golang.rs`, `typescript.rs`: `format_expr_*`
- 新しい `Item` バリアントを追加する場合、以下の全箇所に match アームを追加:
  - `src/main.rs`: `load_and_prepare`, `cmd_check`, `cmd_build`
  - `src/resolver.rs`: `resolve_imports_recursive`, `register_imported_items`
  - `src/lsp.rs`: `verify_source_for_lsp`
- `#[allow(dead_code)]` を使う場合は NOTE コメントで理由を記述

## ビルド & テスト

```bash
./build_and_run.sh          # ビルド + テストスイート実行
cargo test                  # Rust ユニットテスト（パーサーテスト含む）
mumei verify sword_test.mm  # メイン検証スイート
```
