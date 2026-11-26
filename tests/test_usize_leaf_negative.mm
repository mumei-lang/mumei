// Wrong facts about `usize`/`isize` leaves must refute.
struct Bucket {
    total: usize
}

// `a < 0` is unsatisfiable for a `usize`: a requires clause demanding
// it must not produce a vacuous "verified" atom.
atom vacuous(a: usize) -> i64
requires: a < 0;
ensures: result == 0;
body: {
    0
};

// A wrong postcondition over usize arithmetic refutes.
atom bad_add(a: usize, b: usize) -> usize
requires: a == 1 && b == 1;
ensures: result == 3;
body: {
    a + b
};

// A declared usize cannot masquerade as i64 through the comparison:
// `a - 1` under signed reading at a == 0 would violate `a >= 0`.
atom underflow(a: usize) -> usize
requires: a == 0;
ensures: result == 1;
body: {
    a - 1
};

atom main() -> i64
requires: true;
ensures: result == 1;
body: {
    1
};
