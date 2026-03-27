// Expected: verification failure — index out of bounds
// vec_get requires: len > 0 && index >= 0 && index < len
// but caller only guarantees len >= 0, no bounds on index
atom vec_get_checked(vec_len: i64, index: i64)
    requires: vec_len > 0 && index >= 0 && index < vec_len;
    ensures: result >= 0;
    body: { index }

atom unsafe_vec_access(len: i64, index: i64)
    requires: len >= 0;
    ensures: result >= 0;
    body: {
        vec_get_checked(len, index)
    }
