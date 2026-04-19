// Atom: abs_saturating
// |x| を返す。x == i64::MIN の場合は i64::MAX に飽和する。
atom abs_saturating(x: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        if x == (0 - 9223372036854775807 - 1) { 9223372036854775807 }
        else { if x >= 0 { x } else { 0 - x } }
    };