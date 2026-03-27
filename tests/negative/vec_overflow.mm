// Expected: verification failure — push without capacity check
// alloc.vec_push requires: vec_len >= 0 && vec_cap > 0 && vec_len < vec_cap
// but caller only guarantees len >= 0 && cap > 0 (missing len < cap)
import "std/alloc" as alloc;

atom unsafe_push(len: i64, cap: i64)
    requires: len >= 0 && cap > 0;
    ensures: result >= 0;
    body: {
        alloc.vec_push(len, cap)
    }
