// =============================================================
// std/math/clamp — verified clamping primitives
// =============================================================
// 範囲制限と飽和を行う基本的な clamp atom 群。

atom clamp_i64(val: i64, min_val: i64, max_val: i64)
requires: min_val <= max_val;
ensures: result >= min_val && result <= max_val;
body: {
    if val < min_val { min_val } else { if val > max_val { max_val } else { val } }
};

atom clamp_nonneg(val: i64, max_val: i64)
requires: max_val >= 0;
ensures: result >= 0 && result <= max_val;
body: {
    if val < 0 { 0 } else { if val > max_val { max_val } else { val } }
};

atom clamp_symmetric(val: i64, bound: i64)
requires: bound >= 0;
ensures: result >= 0 - bound && result <= bound;
body: {
    if val < 0 - bound { 0 - bound } else { if val > bound { bound } else { val } }
};

atom clamp_saturating(val: i64, min_val: i64, max_val: i64)
requires: true;
ensures: true;
body: {
    if min_val > max_val { min_val } else { if val < min_val { min_val } else { if val > max_val { max_val } else { val } } }
};
