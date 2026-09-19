// =============================================================
// std/math/pow_nat — verified small-domain integer powers
// =============================================================
// pow2 enumerates its bounded domain (0..30) as a match — the
// ensures table mirrors the arm table, which keeps both sides in
// the decidable fragment: a `1 << exp` contract is honest but Z3
// 4.8.12 cannot decide `ite-table == bvshl(1, exp)`.

atom pow2(exp: i64)
requires: exp >= 0 && exp <= 30;
ensures: (exp == 0 && result == 1) ||
    (exp == 1 && result == 2) ||
    (exp == 2 && result == 4) ||
    (exp == 3 && result == 8) ||
    (exp == 4 && result == 16) ||
    (exp == 5 && result == 32) ||
    (exp == 6 && result == 64) ||
    (exp == 7 && result == 128) ||
    (exp == 8 && result == 256) ||
    (exp == 9 && result == 512) ||
    (exp == 10 && result == 1024) ||
    (exp == 11 && result == 2048) ||
    (exp == 12 && result == 4096) ||
    (exp == 13 && result == 8192) ||
    (exp == 14 && result == 16384) ||
    (exp == 15 && result == 32768) ||
    (exp == 16 && result == 65536) ||
    (exp == 17 && result == 131072) ||
    (exp == 18 && result == 262144) ||
    (exp == 19 && result == 524288) ||
    (exp == 20 && result == 1048576) ||
    (exp == 21 && result == 2097152) ||
    (exp == 22 && result == 4194304) ||
    (exp == 23 && result == 8388608) ||
    (exp == 24 && result == 16777216) ||
    (exp == 25 && result == 33554432) ||
    (exp == 26 && result == 67108864) ||
    (exp == 27 && result == 134217728) ||
    (exp == 28 && result == 268435456) ||
    (exp == 29 && result == 536870912) ||
    (exp == 30 && result == 1073741824);
body: {
        match exp {
            0 => 1,
            1 => 2,
            2 => 4,
            3 => 8,
            4 => 16,
            5 => 32,
            6 => 64,
            7 => 128,
            8 => 256,
            9 => 512,
            10 => 1024,
            11 => 2048,
            12 => 4096,
            13 => 8192,
            14 => 16384,
            15 => 32768,
            16 => 65536,
            17 => 131072,
            18 => 262144,
            19 => 524288,
            20 => 1048576,
            21 => 2097152,
            22 => 4194304,
            23 => 8388608,
            24 => 16777216,
            25 => 33554432,
            26 => 67108864,
            27 => 134217728,
            28 => 268435456,
            29 => 536870912,
            _ => 1073741824
        }
    };

// exp is bounded to 0..6, so the exact value is a finite case split.
atom pow_nat(base: i64, exp: i64)
requires: base >= 0 && base <= 10 && exp >= 0 && exp <= 6;
ensures: (exp == 0 && result == 1)
    || (exp == 1 && result == base)
    || (exp == 2 && result == base * base)
    || (exp == 3 && result == base * base * base)
    || (exp == 4 && result == base * base * base * base)
    || (exp == 5 && result == base * base * base * base * base)
    || (exp == 6 && result == base * base * base * base * base * base);
body: {
    if exp == 0 { 1 } else {
    if exp == 1 { base } else {
    if exp == 2 { base * base } else {
    if exp == 3 { base * base * base } else {
    if exp == 4 { base * base * base * base } else {
    if exp == 5 { base * base * base * base * base } else {
        base * base * base * base * base * base
    }}}}
    }};
