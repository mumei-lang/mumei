// A `u64` payload leaf keeps its declared unsigned type: `x + y` against
// a declared `i64` param is the mixing rule's reject, not a silent
// signed fold. Before `local_int_types` saw match-bound leaves this
// atom verified under signed `i64` semantics — divergent from LLVM's
// unsigned ops.

enum W {
    A(u64),
    B
}

atom mix(e: W, y: i64) -> i64
requires: true;
ensures: result >= 0;
body: {
    match e {
        W::A(x) => x + y,
        _ => 0
    }
}
