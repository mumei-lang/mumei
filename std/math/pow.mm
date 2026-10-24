// =============================================================
// std/math/pow — verified integer exponentiation
// =============================================================
// 小さい指数と飽和境界を明示分岐で検証する。

atom pow_small(base: i64, exp: i64)
requires: base >= 0 && base <= 1000 && exp >= 0 && exp <= 3;
ensures: (exp == 0 && result == 1)
    || (exp == 1 && result == base)
    || (exp == 2 && result == base * base)
    || (exp == 3 && result == base * base * base);
body: {
    if exp == 0 { 1 } else { if exp == 1 { base } else { if exp == 2 { base * base } else { base * base * base } } }
};

atom pow_saturating(base: i64, exp: i64, max_val: i64)
requires: base >= 0 && base <= 1000 && exp >= 0 && exp <= 3 && max_val >= 0;
ensures: result >= 0 && result <= max_val
    && (exp != 0 || (1 <= max_val && result == 1) || (max_val < 1 && result == max_val))
    && (exp != 1 || (base <= max_val && result == base) || (base > max_val && result == max_val))
    && (exp != 2 || (base * base <= max_val && result == base * base) || (base * base > max_val && result == max_val))
    && (exp != 3 || (base * base * base <= max_val && result == base * base * base) || (base * base * base > max_val && result == max_val));
body: {
    let p = if exp == 0 { 1 } else { if exp == 1 { base } else { if exp == 2 { base * base } else { base * base * base } } };
    if p <= max_val { p } else { max_val }
};
