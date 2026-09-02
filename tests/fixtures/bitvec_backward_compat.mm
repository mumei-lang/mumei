// Backward-compatibility fixture for the opt-in `--bitvec-i64` mode (P10-A).
//
// None of these atoms uses bitwise syntax or `semantics: bitvec`, so they must
// keep being verified with the default Z3 `Int` encoding. The golden file
// `bitvec_backward_compat.golden.json` pins their certificate fields, so any
// change that leaks bit-vector semantics into default verification (different
// proof hash, `BitVec 64` binders, bit-vector lowering rules) fails the gate.

atom bc_add(a: i64, b: i64) -> i64
    requires: a >= 0 && a <= 1000 && b >= 0 && b <= 1000;
    ensures: result == a + b && result >= 0 && result <= 2000;
    body: {
        a + b
    };

atom bc_double(x: i64) -> i64
    requires: x >= 0 && x <= 100;
    ensures: result == x * 2;
    body: {
        x * 2
    };

atom bc_clamp(x: i64, lo: i64, hi: i64) -> i64
    requires: lo <= hi;
    ensures: result >= lo && result <= hi;
    body: {
        if x < lo { lo } else { if x > hi { hi } else { x } }
    };
