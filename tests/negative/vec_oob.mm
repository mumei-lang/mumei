// Expected: verification failure — index out of bounds
// alloc.vec_get requires: vec_len > 0 && index >= 0 && index < vec_len
// but caller only guarantees len >= 0, no bounds on index
import "std/alloc" as alloc;

atom unsafe_vec_access(len: i64, index: i64)
    requires: len >= 0;
    ensures: result >= 0;
    body: {
        alloc.vec_get(len, index)
    }
