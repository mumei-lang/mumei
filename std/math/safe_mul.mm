// =============================================================
// std/math/safe_mul — verified multiplication
// =============================================================
// 非負整数の乗算を明示的な境界条件付きで検証する。

// 小さな非負整数の正確な積。
atom safe_mul(a: i64, b: i64)
requires: a >= 0 && b >= 0 && a <= 1000000 && b <= 1000000;
ensures: result == a * b && result >= 0;
body: {
    a * b
};

// 上限を超える場合は max_val に飽和する乗算。
atom saturating_mul(a: i64, b: i64, max_val: i64)
requires: a >= 0 && b >= 0 && max_val >= 0 && a <= 3037000499 && b <= 3037000499;
ensures: result >= 0 && result <= max_val;
body: {
    if a * b > max_val { max_val } else { a * b }
};
