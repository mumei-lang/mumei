// `u64` leaf parameters, fields and results (`pure-i64-composite/v10`).
// A declared `u64` carries the full unsigned machine range; comparisons
// and division read unsigned on both the verifier and the emitted side.
// Literals adapt to the declared leaf — no suffix notation is needed
// (the lexer caps literals at i64::MAX; `0 - 1` denotes u64::MAX).

// Unsigned comparison: under signed semantics `a >= 0` would be
// refutable with a = -1; under unsigned it is trivially true.
atom ge_zero(a: u64) -> i64
requires: a >= 0;
ensures: result == 1;
body: {
    1
};

// `0 - 1` is u64::MAX under unsigned wrap semantics.
atom wrap_max(a: u64) -> u64
requires: a == 0;
ensures: result == 0 - 1;
body: {
    a - 1
};

// `a + 1` wraps to zero at `u64::MAX` in the machine-word model.
atom succ_wraps(a: u64) -> u64
requires: a == 0 - 1;
ensures: result == 0;
body: {
    a + 1
};

// Unsigned division/remainder: no `i64::MIN / -1` analog exists.
atom halve(a: u64) -> u64
requires: a % 2 == 0;
ensures: result * 2 == a;
body: {
    a / 2
};

// A u64 field inside a struct.
struct Bucket {
    total: u64,
    count: i64
}

atom add_total(b: Bucket, extra: u64) -> Bucket
requires: b.total >= 0 && extra >= 0;
ensures: result.total == b.total + extra && result.count == b.count;
body: {
    Bucket{total: b.total + extra, count: b.count}
};

atom main() -> i64
requires: true;
ensures: result == 1;
body: {
    ge_zero(7)
};
