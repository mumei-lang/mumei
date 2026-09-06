// =============================================================
// Mumei Standard Library: Fixed-Point Arithmetic
// =============================================================
// 4 桁の小数精度を持つ固定小数点演算モジュール。
// スケールファクター: 10000（例: 1.5 = 15000）
// オーバーフロー防止と Z3 による検証済み契約を提供する。
//
// Usage: import "std/math/fixed_point" as fp;

struct FixedPoint {
    value: i64 where v >= -999999999999 && v <= 999999999999
}

// =============================================================
// Atoms: Arithmetic（算術演算）
// =============================================================

// 固定小数点加算（オーバーフロー防止）
atom fp_add(a: i64, b: i64)
    requires: a >= -999999999999 && a <= 999999999999
           && b >= -999999999999 && b <= 999999999999
           && a + b >= -999999999999 && a + b <= 999999999999;
    ensures: result >= -999999999999 && result <= 999999999999
          && result == a + b;
    body: {
        a + b
    };

// 固定小数点減算（オーバーフロー防止）
atom fp_sub(a: i64, b: i64)
    requires: a >= -999999999999 && a <= 999999999999
           && b >= -999999999999 && b <= 999999999999
           && a - b >= -999999999999 && a - b <= 999999999999;
    ensures: result >= -999999999999 && result <= 999999999999
          && result == a - b;
    body: {
        a - b
    };

// 固定小数点乗算: result = a * b / 10000
// 中間値 a * b は i64 を超え得るため、以前は「入力を ±10^9 以下に制限せよ」と
// いう手動の目安をコメントで示すだけだった（`ensures: true` は空約束）。
// `semantics: bitvec` により i64 を BV(64)（2の補数ラップ）として検証し、
// 「中間積がラップしない」ことを厳密な条件として requires に置く。
atom fp_mul(a: i64, b: i64)
    semantics: bitvec;
    requires: a >= -999999999999 && a <= 999999999999
           && b >= -999999999999 && b <= 999999999999
           && (a == 0 || b == 0 || ((a * b) / a == b && (a * b) / b == a));
    ensures: result == (a * b) / 10000;
    body: {
        (a * b) / 10000
    };

// 固定小数点除算: result = a * 10000 / b
// a は固定小数点領域（|a| <= 10^12）なので a * 10000 は最大 10^16 で
// i64 内に収まり、手動のオーバーフロー境界は不要。
atom fp_div(a: i64, b: i64)
    requires: a >= -999999999999 && a <= 999999999999
           && b != 0;
    ensures: result == (a * 10000) / b;
    body: {
        (a * 10000) / b
    };

// =============================================================
// Atoms: Conversion（変換）
// =============================================================

// 整数を固定小数点に変換（n * 10000）
atom fp_from_int(n: i64)
    requires: n >= -99999999 && n <= 99999999;
    ensures: result == n * 10000;
    body: {
        n * 10000
    };

// 固定小数点を整数に変換（fp_val / 10000）
atom fp_to_int(fp_val: i64)
    requires: fp_val >= -999999999999 && fp_val <= 999999999999;
    ensures: true;
    body: {
        fp_val / 10000
    };

// =============================================================
// Atoms: Predicates（述語）
// =============================================================

// 正の値かどうかチェック（0=false, 1=true）
atom fp_is_positive(fp_val: i64)
    requires: fp_val >= -999999999999 && fp_val <= 999999999999;
    ensures: result >= 0 && result <= 1;
    body: {
        if fp_val > 0 { 1 } else { 0 }
    };

// 絶対値を返す
atom fp_abs(fp_val: i64)
    requires: fp_val >= -999999999999 && fp_val <= 999999999999;
    ensures: result >= 0;
    body: {
        if fp_val >= 0 { fp_val } else { 0 - fp_val }
    };

// 符号反転: result = -fp_val（オーバーフローしない範囲内で保証）
atom fp_negate(fp_val: i64)
    requires: fp_val >= -999999999999 && fp_val <= 999999999999;
    ensures: result >= -999999999999 && result <= 999999999999
          && result == 0 - fp_val;
    body: {
        0 - fp_val
    };

// 2 つの固定小数点値の最小値
atom fp_min(a: i64, b: i64)
    requires: a >= -999999999999 && a <= 999999999999
           && b >= -999999999999 && b <= 999999999999;
    ensures: result <= a && result <= b;
    body: {
        if a <= b { a } else { b }
    };

// 2 つの固定小数点値の最大値
atom fp_max(a: i64, b: i64)
    requires: a >= -999999999999 && a <= 999999999999
           && b >= -999999999999 && b <= 999999999999;
    ensures: result >= a && result >= b;
    body: {
        if a >= b { a } else { b }
    };

// 指定区間に固定小数点値をクランプする
atom fp_clamp(fp_val: i64, min_val: i64, max_val: i64)
    requires: fp_val >= -999999999999 && fp_val <= 999999999999
           && min_val >= -999999999999 && min_val <= 999999999999
           && max_val >= -999999999999 && max_val <= 999999999999
           && min_val <= max_val;
    ensures: result >= min_val && result <= max_val;
    body: {
        if fp_val < min_val { min_val }
        else { if fp_val > max_val { max_val } else { fp_val } }
    };
