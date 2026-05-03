// --- 正常系: 完全決済フロー ---
// Pending → Validated → Settled の正常遷移

effect Settlement
    states: [Pending, Validated, Settled];
    initial: Pending;
    transition validate: Pending -> Validated;
    transition settle: Validated -> Settled;
    transition reject: Pending -> Pending;

resource ledger priority: 1 mode: exclusive;
resource queue  priority: 2 mode: exclusive;

// 正常: validate → settle の順序
atom test_valid_settlement(sender_bal: i64, receiver_bal: i64, amount: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Pending };
    effect_post: { Settlement: Settled };
    requires: sender_bal >= 100 && receiver_bal >= 0 && amount == 50;
    ensures: result == sender_bal + receiver_bal;
    body: {
        perform Settlement.validate;
        perform Settlement.settle;
        let new_sender = sender_bal - amount;
        let new_receiver = receiver_bal + amount;
        new_sender + new_receiver
    };

// 残高保存の検証: sender + receiver は送金前後で不変
atom test_balance_conservation(s: i64, r: i64, a: i64)
    requires: s >= 0 && r >= 0 && a > 0 && s >= a;
    ensures: result == s + r;
    body: {
        let new_s = s - a;
        let new_r = r + a;
        new_s + new_r
    };

// リソース階層: ledger (1) → queue (2) の正しい順序
atom test_resource_order(amount: i64)
    resources: [ledger, queue];
    requires: amount >= 0;
    ensures: result == amount;
    body: {
        acquire ledger {
            acquire queue {
                amount
            }
        }
    };

// キュー処理の停止性
atom test_queue_processing(n: i64)
    requires: n >= 0;
    ensures: result >= 0;
    body: {
        let count = 0;
        let i = 0;
        while i < n
        invariant: i >= 0 && i <= n && count == i
        decreases: n - i
        {
            count = count + 1;
            i = i + 1;
        };
        count
    };

// forall: 全残高が非負
atom test_all_non_negative(n: i64)
    requires: n >= 0 && forall(i, 0, n, arr[i] >= 0);
    ensures: result == 1;
    body: 1;
