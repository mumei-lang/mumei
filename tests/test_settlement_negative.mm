// ❌ Pending から直接 Settled への遷移（validate をスキップ）
// → InvalidPreState エラーが期待される

effect Settlement
    states: [Pending, Validated, Settled];
    initial: Pending;
    transition validate: Pending -> Validated;
    transition settle: Validated -> Settled;
    transition reject: Pending -> Pending;

atom hostile_settlement(amount: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Pending };
    effect_post: { Settlement: Settled };
    requires: amount > 0;
    ensures: result == amount;
    body: {
        // BUG: validate をスキップして直接 settle
        perform Settlement.settle;
        amount
    };
