// =============================================================
// std/math/safe_mul — verified multiplication
// =============================================================
// 非負整数の乗算を明示的な境界条件付きで検証する。

// 非負整数の正確な積。手動の ±10^6 境界ではなく、ラップした積からは
// 元のオペランドを復元できないことを利用した厳密な条件を使う。
atom safe_mul(a: i64, b: i64)
semantics: bitvec;
requires: a >= 0 && b >= 0 && (a == 0 || (a * b) / a == b);
ensures: result == a * b && result >= 0;
body: {
    a * b
};

// 上限を超える場合は max_val に飽和する乗算。手動の sqrt(i64::MAX)
// 境界（±3037000499）を積のラップ判定に置き換える。
atom saturating_mul(a: i64, b: i64, max_val: i64)
semantics: bitvec;
requires: a >= 0 && b >= 0 && max_val >= 0 && (a == 0 || (a * b) / a == b);
ensures: result >= 0 && result <= max_val;
body: {
    if a * b > max_val { max_val } else { a * b }
};
