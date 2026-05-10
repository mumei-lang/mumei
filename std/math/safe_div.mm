// =============================================================
// std/math/safe_div — verified integer division
// =============================================================
// 非負整数の除算と剰余で、除数が正であることを requires で保証する。

// 正の除数による安全な整数除算。
atom safe_div(a: i64, b: i64)
requires: a >= 0 && b > 0;
ensures: result >= 0 && result <= a && result * b <= a;
body: {
    a / b
};

// 正の除数による安全な剰余範囲ヘルパー。
atom safe_mod(a: i64, b: i64)
requires: a >= 0 && b > 0;
ensures: result >= 0 && result < b;
body: {
    a - (a / b) * b
};
