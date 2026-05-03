// =============================================================
// tests/test_settlement.mm — RTGS Settlement Protocol E2E Test
// =============================================================
// Integration test for std/settlement.mm verified contracts.
// Usage: mumei check tests/test_settlement.mm
// Verification: mumei verify tests/test_settlement.mm

import "std/settlement" as settlement;

// 正常: validate → settle を合成する safe_settlement を経由
atom test_valid_settlement(sender_bal: i64, receiver_bal: i64, amount: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Pending };
    effect_post: { Settlement: Settled };
    requires: sender_bal >= 100 && receiver_bal >= 0 && amount == 50;
    ensures: result == sender_bal + receiver_bal;
    body: {
        settlement::safe_settlement(sender_bal, receiver_bal, amount)
    };

// 残高保存: validate_transaction と execute_settlement の合成で保証される
atom test_balance_conservation(s: i64, r: i64, a: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Pending };
    effect_post: { Settlement: Settled };
    requires: s >= 0 && r >= 0 && a > 0 && s >= a;
    ensures: result == s + r;
    body: {
        settlement::safe_settlement(s, r, a)
    };

// リソース階層: full_settlement が ledger (1) → queue (2) の順序を強制
atom test_resource_order(sender_bal: i64, receiver_bal: i64, amount: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Pending };
    effect_post: { Settlement: Settled };
    resources: [ledger, queue];
    requires: sender_bal >= 0 && receiver_bal >= 0 && amount > 0 && sender_bal >= amount;
    ensures: result == sender_bal + receiver_bal;
    body: {
        settlement::full_settlement(sender_bal, receiver_bal, amount)
    };

// キュー処理の停止性: std atom の process_queue を呼ぶ
atom test_queue_processing(n: i64, total_balance: i64)
    requires: n >= 0 && total_balance >= 0;
    ensures: result >= 0;
    body: {
        settlement::process_queue(n, total_balance)
    };

// forall: 全残高が非負であることを std atom で検証
atom test_all_non_negative(n: i64)
    requires: n >= 0 && forall(i, 0, n, arr[i] >= 0);
    ensures: result == 1;
    body: {
        settlement::verify_all_balances_non_negative(n)
    };
