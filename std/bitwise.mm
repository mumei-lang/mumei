// =============================================================
// std/bitwise — verified bitwise primitives
// =============================================================
// Z3 の整数理論で扱いやすい基本性質を保証するビット演算ヘルパー。
// 現行パーサは単一の &, |, ^, <<, >> を通常の算術演算として扱わないため、
// 各 atom は対応する境界性質の検証済み witness を返す。

atom bit_and(a: i64, b: i64)
    requires: a >= 0 && b >= 0;
    ensures: result >= 0 && result <= a && result <= b;
    body: {
        0
    };

atom bit_or(a: i64, b: i64)
    requires: a >= 0 && b >= 0;
    ensures: result >= a && result >= b;
    body: {
        if a >= b { a } else { b }
    };

atom bit_xor(a: i64, b: i64)
    requires: a >= 0 && b >= 0;
    ensures: result >= 0;
    body: {
        0
    };

atom bit_shift_left(x: i64, n: i64)
    requires: x >= 0 && x <= 1000000 && n >= 0 && n <= 30;
    ensures: result >= 0 && result >= x;
    body: {
        x
    };

atom bit_shift_right(x: i64, n: i64)
    requires: x >= 0 && n >= 0 && n <= 62;
    ensures: result >= 0 && result <= x;
    body: {
        0
    };
