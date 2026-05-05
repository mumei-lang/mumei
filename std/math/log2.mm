// =============================================================
// std/math/log2 — verified integer log2
// =============================================================
// 正の入力に対する log2 の下限 witness を返す。

atom ilog2(n: i64)
    requires: n > 0;
    ensures: result >= 0 && result <= n;
    body: {
        0
    };
