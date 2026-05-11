// =============================================================
// Mumei Standard Library: Math Proof Patterns
// =============================================================
// 金融・決済・スマートコントラクトで頻出する境界・変換・保存則の
// 検証済み atom。Lean 側の MumeiLean.Patterns と対応する。

// 乗算結果を明示的な上限で保護する。
atom bounded_mul_with_overflow_check(a: i64, b: i64, limit: i64)
    requires: a >= 0 && b >= 0 && limit >= 0 && a <= limit && b <= limit && a * b <= limit;
    ensures: result >= 0 && result <= limit && result == a * b;
    body: {
        a * b
    };

// 値を指定範囲にクランプし、結果が範囲内であることを保証する。
atom clamp_preserves_order(x: i64, min_val: i64, max_val: i64)
    requires: min_val <= max_val;
    ensures: result >= min_val && result <= max_val;
    body: {
        if x < min_val { min_val }
        else { if x > max_val { max_val } else { x } }
    };

// スケール変換後に元の値へ戻る round-trip 性質を検証する。
atom round_trip_conversion(x: i64, scale: i64, min_val: i64, max_val: i64)
    requires: scale > 0 && min_val <= x && x <= max_val && (x * scale) / scale == x;
    ensures: result == x && result >= min_val && result <= max_val;
    body: {
        (x * scale) / scale
    };

// 2口座間の資金移動で合計残高が保存されることを表す。
atom sum_invariant(from_balance: i64, to_balance: i64, amount: i64)
    requires: amount >= 0 && from_balance >= amount && to_balance >= 0;
    ensures: result == from_balance + to_balance;
    body: {
        let new_from = from_balance - amount;
        let new_to = to_balance + amount;
        new_from + new_to
    };

// 条件を満たす件数から1件を安全に消費する単純な count 保存パターン。
atom count_invariant(count_before: i64, removed: i64, added: i64)
    requires: count_before >= 0 && removed >= 0 && added >= 0 && removed <= count_before && removed == added;
    ensures: result == count_before && result >= 0;
    body: {
        count_before - removed + added
    };
