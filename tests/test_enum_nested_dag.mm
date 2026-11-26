// A deep shared-subtree payload: each Sn has two identical

// children, so a naive leaf walk visits 2^n nodes. All leaves are

// empty structs, so the leaf cap never trips — only the visit

// budget bounds the walk (regression for exponential flattening).

struct E {
}

struct S1 {
    a: E,
    b: E
}

struct S2 {
    a: S1,
    b: S1
}

struct S3 {
    a: S2,
    b: S2
}

struct S4 {
    a: S3,
    b: S3
}

struct S5 {
    a: S4,
    b: S4
}

struct S6 {
    a: S5,
    b: S5
}

struct S7 {
    a: S6,
    b: S6
}

struct S8 {
    a: S7,
    b: S7
}

struct S9 {
    a: S8,
    b: S8
}

struct S10 {
    a: S9,
    b: S9
}

struct S11 {
    a: S10,
    b: S10
}

struct S12 {
    a: S11,
    b: S11
}

struct S13 {
    a: S12,
    b: S12
}

struct S14 {
    a: S13,
    b: S13
}

struct S15 {
    a: S14,
    b: S14
}

struct S16 {
    a: S15,
    b: S15
}

struct S17 {
    a: S16,
    b: S16
}

struct S18 {
    a: S17,
    b: S17
}

struct S19 {
    a: S18,
    b: S18
}

struct S20 {
    a: S19,
    b: S19
}

struct S21 {
    a: S20,
    b: S20
}

struct S22 {
    a: S21,
    b: S21
}

struct S23 {
    a: S22,
    b: S22
}

struct S24 {
    a: S23,
    b: S23
}

struct S25 {
    a: S24,
    b: S24
}

struct S26 {
    a: S25,
    b: S25
}

struct S27 {
    a: S26,
    b: S26
}

struct S28 {
    a: S27,
    b: S27
}

struct S29 {
    a: S28,
    b: S28
}

struct S30 {
    a: S29,
    b: S29
}

enum DE {
    W(x: S30),
    N
}

atom dag_probe(e: DE) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        match e {
            DE::W(x) => 0,
            _ => 1
        }
    }
