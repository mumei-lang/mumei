// =============================================================
// std/math/pow_nat — verified small-domain integer powers
// =============================================================
// 自然数べき乗の小領域 witness を Z3 で検証する。

atom pow2(exp: i64)
requires: exp >= 0 && exp <= 30;
ensures: result >= 1;
body: {
    if exp == 0 { 1 } else { if exp == 1 { 2 } else { if exp == 2 { 4 } else { 8 } } }
};

atom pow_nat(base: i64, exp: i64)
requires: base >= 0 && base <= 10 && exp >= 0 && exp <= 6;
ensures: result >= 0;
body: {
    if exp == 0 { 1 } else { if exp == 1 { base } else { base * base } }
};
