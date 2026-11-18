// Wrong facts about `u64` leaves must refute, not vacuously pass.
struct Bucket {
    total: u64
}

// `a < 0` is unsatisfiable for a `u64`: a requires clause demanding it
// must not produce a vacuous "verified" atom.
atom vacuous(a: u64) -> i64
requires: a < 0;
ensures: result == 0;
body: {
    0
};

// A wrong postcondition over u64 arithmetic refutes.
atom bad_add(a: u64, b: u64) -> u64
requires: a == 1 && b == 1;
ensures: result == 3;
body: {
    a + b
};

// `a + 1 > a` does not hold for a machine-word `u64`: at `u64::MAX` it wraps.
atom bad_wrap(a: u64) -> u64
requires: a == 0 - 1;
ensures: result > a;
body: {
    a + 1
};

// A declared u64 must not mix with a declared non-u64 operand.
atom mixed(a: u64, b: i64) -> i64
requires: true;
ensures: result == 0;
body: {
    if a < b { 0 } else { 1 }
};

atom main() -> i64
requires: true;
ensures: result == 0;
body: {
    0
};
