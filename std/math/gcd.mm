// =============================================================
// std/math/gcd — verified GCD primitives
// =============================================================
// Euclid 法の基底ケース、1 ステップ、境界 witness を検証する。

atom gcd_base_zero(a: i64)
requires: a >= 0;
ensures: result >= 0 && result == a;
body: {
    a
};

atom gcd_step(a: i64, b: i64)
requires: a >= 0 && b > 0;
ensures: result >= 0 && result < b;
body: {
    a - (a / b) * b
};

atom gcd_of_equal(a: i64)
requires: a >= 0;
ensures: result >= 0 && result == a;
body: {
    a
};

atom gcd_bound(a: i64, b: i64)
requires: a > 0 && b > 0;
ensures: result > 0 && result <= a && result <= b;
body: {
    if a <= b { a } else { b }
};

atom lcm_from_gcd(qa: i64, b: i64)
requires: qa >= 0 && qa <= 1000000 && b >= 0 && b <= 1000000;
ensures: result >= 0;
body: {
    qa * b
};
