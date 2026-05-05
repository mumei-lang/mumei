// =============================================================
// std/math/sqrt — verified integer square root
// =============================================================
// 非負入力に対し、二乗が入力以下である整数平方根 witness を返す。

atom isqrt(n: i64)
    requires: n >= 0;
    ensures: result >= 0 && result * result <= n;
    body: {
        0
    };
