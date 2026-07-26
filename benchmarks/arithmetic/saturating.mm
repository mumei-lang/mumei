// Saturating arithmetic: results clamp instead of wrapping.
// expected: PASS

atom saturating_add_u16(a: i64, b: i64)
requires: a >= 0 && a <= 65535 && b >= 0 && b <= 65535;
ensures: result >= 0 && result <= 65535 && (result == a + b || result == 65535);
body: { if a + b > 65535 { 65535 } else { a + b } };

atom saturating_sub_u16(a: i64, b: i64)
requires: a >= 0 && a <= 65535 && b >= 0 && b <= 65535;
ensures: result >= 0 && result <= 65535 && (result == a - b || result == 0);
body: { if a < b { 0 } else { a - b } };

atom clamp_to_range(x: i64, lo: i64, hi: i64)
requires: lo <= hi;
ensures: result >= lo && result <= hi && (result == x || result == lo || result == hi);
body: { if x < lo { lo } else { if x > hi { hi } else { x } } };
