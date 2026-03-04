# Structured Concurrency Design Document

> Mumei の構造化並行性と Z3 検証戦略

## 概要

Mumei は **構造化並行性 (Structured Concurrency)** を採用し、
タスクのライフサイクルを型システムと Z3 ソルバで形式的に保証します。
親タスクが子タスクより先に終了しないことをコンパイル時に検証することで、
ダングリングタスクやリソースリークを防止します。

## 既存の非同期基盤

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

### リソース定義

```mumei
resource db_conn priority: 1 mode: exclusive;
resource cache   priority: 2 mode: shared;
```

## Task 構文 (PR-C)

### task 式

子タスクを生成する。親タスクのスコープ内で実行され、
構造化並行性により親が先に終了しないことが保証される。

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
    group: Option<String>,  // タスクグループ名（省略時はデフォルト）
}
```

## TaskGroup 構文 (PR-C)

### task_group 式

複数の子タスクをグループ化し、Join セマンティクスに従って完了を待機する。

```mumei
// 全タスクの完了を待つ（デフォルト: All）
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

## 安全性保証の一覧

| プロパティ | 検証方法 | ステータス |
|---|---|---|
| デッドロック防止 | リソース階層 (priority) の Z3 検証 | 実装済み |
| await 跨ぎリソース保持 | acquire ブロック内 await 検出 | 実装済み |
| 非同期再帰深度 | BMC 展開上限チェック | 実装済み |
| 親タスク終了制約 | TaskGroup join semantics の Z3 検証 | 設計済み (PR-C) |
| タスクキャンセル安全性 | Any セマンティクスでの残タスククリーンアップ | 将来 |

## 将来の拡展

1. **ランタイムスケジューラ**: プリエンプティブなタスクスケジューリング
2. **チャネル型**: タスク間通信のための型安全チャネル
3. **タスクキャンセル**: `Any` 完了時の残タスクの安全なキャンセル処理
4. **タイムアウト**: タスクグループへのタイムアウト指定
5. **LLVM コード生成**: タスクスケジューリングコードの LLVM IR 生成

## 関連ファイル

- `src/parser.rs` — `Task`, `TaskGroup`, `JoinSemantics` 定義 + パース処理
- `src/verification.rs` — Z3 による構造化並行性検証
- `src/codegen.rs` — `pthread_mutex_lock` ベースの acquire 実装 (既存)
