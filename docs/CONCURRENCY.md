# Structured Concurrency Design Document

> Mumei's structured concurrency and Z3 verification strategy.

## Overview

Mumei adopts **Structured Concurrency**, formally guaranteeing task lifecycle
properties through the type system and Z3 solver.
By verifying at compile time that parent tasks do not terminate before child tasks,
dangling tasks and resource leaks are prevented.

## Existing Async Foundation

### async atom

```mumei
async atom fetch_data(url: String) -> Result<String, Error>
    requires: url.len() > 0;
    ensures: result.is_ok();
    body: ...;
```

### acquire / await

```mumei
acquire db_conn {
    let data = await fetch_data("https://...");
    process(data)
}
```

### Resource Definitions

```mumei
resource db_conn priority: 1 mode: exclusive;
resource cache   priority: 2 mode: shared;
```

## Task 構文 (PR-C)

### task Expression

Spawns a child task. Executes within the parent task's scope,
with structured concurrency guaranteeing the parent does not terminate first.

```mumei
task {
    // 子タスクの body
    compute_heavy_work(data)
}

// グループ名を指定
task workers {
    process_item(item)
}
```

### AST 表現

```rust
Expr::Task {
    body: Box<Expr>,
    group: Option<String>,  // task group name (default if omitted)
}
```

## TaskGroup 構文 (PR-C)

### task_group Expression

Groups multiple child tasks and waits for completion according to Join semantics.

```mumei
// Wait for all tasks to complete (default: All)
task_group {
    task { fetch_users() };
    task { fetch_orders() };
    task { fetch_products() }
}

// 最初の完了で続行（Any）
task_group:any {
    task { primary_server() };
    task { fallback_server() }
}
```

### AST 表現

```rust
Expr::TaskGroup {
    children: Vec<Expr>,
    join_semantics: JoinSemantics,
}

pub enum JoinSemantics {
    All,  // 全タスクの完了を待つ（デフォルト）
    Any,  // 最初に完了したタスクの結果を返す
}
```

## Z3 検証戦略

### 構造化並行性の保証

Z3 ソルバにより以下の安全性プロパティをコンパイル時に検証します:

#### 1. 親タスク終了制約

**制約**: 親タスクが子タスクより先に終了しない

```
JoinSemantics::All の場合:
  parent_done => ∀i. child_done[i]
  （親の完了は全子タスクの完了を前提とする）

JoinSemantics::Any の場合:
  parent_done => ∃i. child_done[i]
  （親の完了は少なくとも1つの子タスク完了を前提とする）
```

#### 2. リソース保持検証 (既存)

`await` ポイントでリソースを保持していないことを検証:

```
acquire ブロック内で await を呼ぶ → デッドロックリスク → エラー
```

#### 3. 所有権一貫性 (既存)

`await` 前に消費済みの変数が `await` 後にアクセスされないことを検証。

### 検証フロー

```
1. task { body } をパース
2. body の安全性を再帰的に Z3 で検証
3. TaskGroup 内の各子タスクを検証
4. Join セマンティクスに応じた終了制約を Z3 solver に assert
5. 制約充足性チェック → 違反時はコンパイルエラー
```

## 実装状況

| 項目 | ステータス |
|---|---|
| `Expr::Task` / `Expr::TaskGroup` AST | ✅ 実装済み |
| `JoinSemantics` enum (All/Any) | ✅ 実装済み |
| `task` / `task_group` パース | ✅ 実装済み (`:all` / `:any` 対応、不正トークン検出) |
| Z3 join 制約 (シンボリック Bool) | ✅ 実装済み (parent_done ⇒ child_done) |
| AST walker 全対応 | ✅ 実装済み (collect_callees, count_self_calls, collect_acquire_resources, collect_from_expr) |
| LLVM codegen | ✅ 実装済み (body を同期的にコンパイル) |
| Rust transpile | ✅ 実装済み (tokio::spawn / tokio::join!) |
| Go transpile | ✅ 実装済み (goroutine + channel) |
| TypeScript transpile | ✅ 実装済み (async/Promise.all) |
| パーサーテスト | ✅ 実装済み (6 テスト: task, task_group, :all, :any, unknown panic) |
| ユニーク ID (Task) | ✅ 実装済み (TASK_COUNTER で env キー衝突防止) |
| ランタイムスケジューラ | ❌ 未実装 |
| タスクキャンセル | ❌ 未実装 |
| チャネル型 | ❌ 未実装 |

## 安全性保証の一覧

| プロパティ | 検証方法 | ステータス |
|---|---|---|
| デッドロック防止 | リソース階層 (priority) の Z3 検証 | ✅ 実装済み |
| await 跨ぎリソース保持 | acquire ブロック内 await 検出 | ✅ 実装済み |
| 非同期再帰深度 | BMC 展開上限チェック | ✅ 実装済み |
| 親タスク終了制約 | TaskGroup join semantics の Z3 検証 | ✅ 実装済み |
| タスクキャンセル安全性 | Any セマンティクスでの残タスククリーンアップ | ❌ 将来 |

## 将来の拡展

> 詳細: [`docs/ROADMAP.md`](ROADMAP.md)

### Roadmap P1-D 関連: std.http との統合

`task_group:all` + HTTP 並行リクエストの統合デモが P1-D で計画されています:

```mumei
import "std/http" as http;

// Concurrent API requests — task_group の実用例
task_group:all {
    task { http.get("https://api.example.com/users") };
    task { http.get("https://api.example.com/orders") };
    task { http.get("https://api.example.com/products") }
}
```

### Task 洗練 (Concurrency Refinement)

1. **ランタイムスケジューラ**: プリエンプティブなタスクスケジューリング
2. **チャネル型**: タスク間通信のための型安全チャネル (`chan<T>`)
3. **タスクキャンセル**: `Any` 完了時の残タスクの安全なキャンセル処理
4. **タイムアウト**: タスクグループへのタイムアウト指定
5. **LLVM コード生成**: タスクスケジューリングコードの LLVM coroutine 変換
6. **TaskGroup ユニーク ID**: 複数 TaskGroup の Z3 変数名衝突防止（TASK_GROUP_COUNTER）
7. **戻り値型推論**: Task の body から戻り値型を自動推論
8. **結果バインド構文**: `task_group` の結果を変数にバインドする構文

## 関連ファイル

- `src/parser.rs` — `Task`, `TaskGroup`, `JoinSemantics` 定義 + パース処理 + テスト
- `src/verification.rs` — Z3 による構造化並行性検証 (シンボリック Bool、join 制約)
- `src/ast.rs` — `collect_from_expr` で Task/TaskGroup 内のジェネリクス走査
- `src/codegen.rs` — Task/TaskGroup の LLVM IR 生成 (同期コンパイル)
- `src/transpiler/rust.rs` — `tokio::spawn` / `tokio::join!`
- `src/transpiler/golang.rs` — goroutine + channel パターン
- `src/transpiler/typescript.rs` — `async` IIFE / `Promise.all`
