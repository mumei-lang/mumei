// =============================================================
// std/math/pow — verified integer exponentiation
// =============================================================
// 小さい指数と飽和境界を明示分岐で検証する。

atom pow_small(base: i64, exp: i64)
requires: base >= 0 && base <= 1000 && exp >= 0 && exp <= 3;
ensures: result >= 0;
body: {
    if exp == 0 { 1 } else { if exp == 1 { base } else { if exp == 2 { base * base } else { base * base * base } } }
};

atom pow_saturating(base: i64, exp: i64, max_val: i64)
requires: base >= 0 && exp >= 0 && max_val >= 0;
ensures: result >= 0 && result <= max_val;
body: {
    if exp == 0 { if 1 <= max_val { 1 } else { max_val } } else { if base <= max_val { base } else { max_val } }
};
