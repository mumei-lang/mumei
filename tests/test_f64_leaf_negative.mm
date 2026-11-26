// `f64` leaves are pass-through only: any arithmetic or comparison
// read of the bit pattern must be refused, not silently modelled.

// A comparison reads the leaf — refused.
atom read_cmp(a: f64) -> i64
requires: true;
ensures: result == 0;
body: {
    if a >= a { 0 } else { 1 }
};

// Bit equality reads the leaf — refused even against itself.
atom read_eq(a: f64) -> i64
requires: true;
ensures: result == 0;
body: {
    if a == a { 0 } else { 1 }
};

// Arithmetic reads the leaf — refused.
atom read_add(a: f64) -> f64
requires: true;
ensures: true;
body: {
    a + a
};

// Mixing f64 with a declared i64 operand is refused.
atom mix(a: f64, b: i64) -> i64
requires: true;
ensures: result == 0;
body: {
    if a >= b { 0 } else { 1 }
};

// A requires clause may not read the leaf either.
atom read_req(a: f64) -> f64
requires: a == a;
ensures: true;
body: {
    a
};

atom main() -> i64
requires: true;
ensures: result == 0;
body: {
    0
};
