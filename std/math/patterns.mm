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

// before/after 配列が各インデックスで等しいとき、合計値も保存される。
atom sum_invariant(before_values: [i64], after_values: [i64], n: i64)
    requires: n >= 0 && len(before_values) >= n && len(after_values) >= n
           && forall(i, 0, n, before_values[i] >= 0 && after_values[i] >= 0 && before_values[i] == after_values[i]);
    ensures: result == 0;
    max_unroll: 5;
    body: {
        let before_sum = 0;
        let after_sum = 0;
        let i = 0;
        while i < n
        invariant: i >= 0 && i <= n && before_sum >= 0 && after_sum >= 0 && before_sum == after_sum
        decreases: n - i
        {
            before_sum = before_sum + before_values[i];
            after_sum = after_sum + after_values[i];
            i = i + 1;
        };
        after_sum - before_sum
    };

// before/after 配列が各インデックスで等しいとき、target 件数も保存される。
atom count_invariant(before_values: [i64], after_values: [i64], n: i64, target: i64)
    requires: n >= 0 && len(before_values) >= n && len(after_values) >= n
           && forall(i, 0, n, before_values[i] == after_values[i]);
    ensures: result == 0;
    max_unroll: 5;
    body: {
        let before_count = 0;
        let after_count = 0;
        let i = 0;
        while i < n
        invariant: i >= 0 && i <= n && before_count >= 0 && before_count <= i && after_count >= 0 && after_count <= i && before_count == after_count
        decreases: n - i
        {
            if before_values[i] == target { before_count = before_count + 1 } else { before_count = before_count };
            if after_values[i] == target { after_count = after_count + 1 } else { after_count = after_count };
            i = i + 1;
        };
        after_count - before_count
    };
