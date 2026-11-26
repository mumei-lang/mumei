// `usize`/`isize` leaf parameters, fields and results
// (`pure-i64-composite/v10`): `usize` folds to `u64` (full unsigned
// machine range — unsigned comparisons, unsigned division, wrap
// arithmetic) and `isize` folds to `i64` (signed range).

// Unsigned comparison on a `usize` leaf.
atom ge_zero(a: usize) -> i64
requires: a >= 0;
ensures: result == 1;
body: {
    1
};

// `0 - 1` wraps to u64::MAX for a `usize` leaf.
atom wrap_max(a: usize) -> usize
requires: a == 0;
ensures: result == 0 - 1;
body: {
    a - 1
};

// Unsigned division on a `usize` leaf.
atom halve(a: usize) -> usize
requires: a % 2 == 0;
ensures: result * 2 == a;
body: {
    a / 2
};

// `isize` is signed: a negative bound is admissible.
atom neg(a: isize) -> isize
requires: a < 0;
ensures: result == a + 1;
body: {
    a + 1
};

// Width-typed leaves inside a struct.
struct Bucket {
    total: usize,
    delta: isize
}

atom add_total(b: Bucket, extra: usize) -> Bucket
requires: b.total >= 0 && extra >= 0;
ensures: result.total == b.total + extra && result.delta == b.delta;
body: {
    Bucket{total: b.total + extra, delta: b.delta}
};

atom main() -> i64
requires: true;
ensures: result == 1;
body: {
    ge_zero(7)
};
