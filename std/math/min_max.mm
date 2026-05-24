// =============================================================
// std/math/min_max — verified min/max primitives
// =============================================================
// i64 値の最小値・最大値を直接比較で返す。

atom min2(a: i64, b: i64)
requires: true;
ensures: result <= a && result <= b && (result == a || result == b);
body: {
    if a <= b { a } else { b }
};

atom max2(a: i64, b: i64)
requires: true;
ensures: result >= a && result >= b && (result == a || result == b);
body: {
    if a >= b { a } else { b }
};

atom min3(a: i64, b: i64, c: i64)
requires: true;
ensures: result <= a && result <= b && result <= c && (result == a || result == b || result == c);
body: {
    if a <= b { if a <= c { a } else { c } } else { if b <= c { b } else { c } }
};
