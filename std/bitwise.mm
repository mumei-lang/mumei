// =============================================================
// std/bitwise — verified bitwise primitives
// =============================================================
// 各 atom は Z3 の Bit-Vector 理論で実ビット意味論として検証される。
// 検証は `mumei verify std/bitwise.mm --bitvec-i64` で行う必要がある
// （`--bitvec-i64` は i64 を BV(64) として符号化する）。
// witness による誤魔化しはなく、ensures は演算の定義そのものである。

atom bit_and(a: i64, b: i64)
    ensures: result == a & b;
    body: {
        a & b
    };

atom bit_or(a: i64, b: i64)
    ensures: result == a | b;
    body: {
        a | b
    };

atom bit_xor(a: i64, b: i64)
    ensures: result == a ^ b;
    body: {
        a ^ b
    };

// シフト量は 0 <= n < 64 が前提（BV のシフトは全域関数だが、
// 範囲外シフトは言語の意味論ではないため requires で除外する）。
atom bit_shift_left(x: i64, n: i64)
    requires: n >= 0 && n < 64;
    ensures: result == x << n;
    body: {
        x << n
    };

// `>>` は算術シフト（符号を伝播する bvashr）。
atom bit_shift_right(x: i64, n: i64)
    requires: n >= 0 && n < 64;
    ensures: result == x >> n;
    body: {
        x >> n
    };
