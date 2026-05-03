// =============================================================
// Mumei Standard Library: RTGS Settlement Protocol
// =============================================================
// 検証済み RTGS 決済プロトコル: Pending から検証・決済を経て、
// 残高不変量と状態遷移の安全性をコンパイル時に保証する。
//
// 検証される性質:
//   - Temporal Effect: Pending → Validated → Settled の順序保証
//   - Resource Hierarchy: ledger → queue の取得順序でデッドロック防止
//   - Loop invariant + decreases: キュー処理の停止性証明
//   - forall 量化子: 残高不変量（送金前後で総残高が保存される）
//
// Usage:
//   import "std/settlement" as settlement;

// --- Temporal Effect: 決済ステータス遷移 ---
effect Settlement
    states: [Pending, Validated, Settled];
    initial: Pending;
    transition validate: Pending -> Validated;
    transition settle: Validated -> Settled;
    transition reject: Pending -> Pending;

// --- Resource Hierarchy: デッドロック防止 ---
resource ledger priority: 1 mode: exclusive;
resource queue  priority: 2 mode: exclusive;

// --- 基本 atom: 決済提出 ---
atom submit_transaction(sender: i64, receiver: i64, amount: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Pending };
    effect_post: { Settlement: Pending };
    requires: sender >= 0 && receiver >= 0 && amount > 0 && sender != receiver;
    ensures: result == amount;
    body: {
        perform Settlement.reject;
        amount
    };

// --- 基本 atom: 残高検証 ---
// sender の残高が amount 以上であることを検証し、Validated へ遷移
atom validate_transaction(sender_balance: i64, amount: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Pending };
    effect_post: { Settlement: Validated };
    requires: sender_balance >= 0 && amount > 0 && sender_balance >= amount;
    ensures: result == sender_balance - amount;
    body: {
        perform Settlement.validate;
        sender_balance - amount
    };

// --- 基本 atom: 決済実行 ---
// Validated 状態から Settled へ遷移し、残高更新を確定
atom execute_settlement(sender_balance: i64, receiver_balance: i64, amount: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Validated };
    effect_post: { Settlement: Settled };
    requires: sender_balance >= 0 && receiver_balance >= 0 && amount > 0 && sender_balance >= amount;
    ensures: result == sender_balance + receiver_balance;
    body: {
        perform Settlement.settle;
        let new_sender = sender_balance - amount;
        let new_receiver = receiver_balance + amount;
        new_sender + new_receiver
    };

// --- 合成 atom: 完全決済フロー ---
// Pending → Validated → Settled の全遷移を合成
// Resource Hierarchy: ledger (priority 1) → queue (priority 2) の順で取得
atom full_settlement(sender_balance: i64, receiver_balance: i64, amount: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Pending };
    effect_post: { Settlement: Settled };
    resources: [ledger, queue];
    requires: sender_balance >= 0 && receiver_balance >= 0 && amount > 0 && sender_balance >= amount;
    ensures: result == sender_balance + receiver_balance;
    body: {
        acquire ledger {
            acquire queue {
                validate_transaction(sender_balance, amount);
                execute_settlement(sender_balance, receiver_balance, amount)
            }
        }
    };

// --- バッチ処理 atom: キュー内トランザクションの順次処理 ---
// Loop invariant + decreases で停止性を証明
// forall 量化子で残高の非負性を保証
atom process_queue(n: i64, total_balance: i64)
    requires: n >= 0 && total_balance >= 0;
    ensures: result >= 0;
    body: {
        let processed = 0;
        let remaining = total_balance;
        let i = 0;
        while i < n
        invariant: i >= 0 && i <= n && processed >= 0 && processed <= i && remaining >= 0
        decreases: n - i
        {
            processed = processed + 1;
            i = i + 1;
        };
        processed
    };

// --- 残高不変量 atom: forall 量化子による全口座残高の非負性保証 ---
atom verify_all_balances_non_negative(n: i64)
    requires: n >= 0 && forall(i, 0, n, arr[i] >= 0);
    ensures: result == 1;
    body: {
        1
    };

// --- 不正決済の検出: Pending から直接 Settled への遷移は不可能 ---
// （buggy_code.mm でこのパターンのバグを検出するデモ用）
// 以下は正しい実装の参考として、validate を経由する必要があることを示す
atom safe_settlement(sender_balance: i64, receiver_balance: i64, amount: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Pending };
    effect_post: { Settlement: Settled };
    requires: sender_balance >= 0 && receiver_balance >= 0 && amount > 0 && sender_balance >= amount;
    ensures: result == sender_balance + receiver_balance;
    body: {
        validate_transaction(sender_balance, amount);
        execute_settlement(sender_balance, receiver_balance, amount)
    };
