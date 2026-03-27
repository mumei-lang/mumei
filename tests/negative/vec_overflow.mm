// Expected: verification failure — push without capacity check
// vec_push requires: len >= 0 && cap > 0 && len < cap
// but caller only guarantees len >= 0 && cap > 0 (missing len < cap)
atom vec_push_checked(vec_len: i64, vec_cap: i64)
    requires: vec_len >= 0 && vec_cap > 0 && vec_len < vec_cap;
    ensures: result >= 0 && result <= vec_cap && result == vec_len + 1;
    body: { vec_len + 1 }

atom unsafe_push(len: i64, cap: i64)
    requires: len >= 0 && cap > 0;
    ensures: result >= 0;
    body: {
        vec_push_checked(len, cap)
    }
