// =============================================================
// std/math/fibonacci — verified fibonacci helpers
// =============================================================
// フィボナッチ列の隣接値更新とループ残数の減少を検証する。

// 隣接する非負値から次の値を計算する。
atom fib_step_next(a: i64, b: i64)
requires: a >= 0 && b >= 0 && a + b <= 9223372036854775807;
ensures: result == a + b && result >= b;
body: {
    a + b
};

// ループの残り回数が 1 減り、非負のままであることを示す。
atom fib_remaining_decreases(remaining: i64)
requires: remaining > 0;
ensures: result == remaining - 1 && result >= 0 && result < remaining;
body: {
    remaining - 1
};
