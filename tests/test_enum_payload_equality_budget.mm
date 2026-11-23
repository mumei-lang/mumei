struct Point {
    x: i64
}

enum ArrayPayload {
    N,
    D(ps: [Point])
}

enum Wrap1 {
    N,
    D(a: ArrayPayload, b: ArrayPayload, c: ArrayPayload, d: ArrayPayload, e: ArrayPayload)
}

enum Wrap2 {
    N,
    D(a: Wrap1, b: Wrap1, c: Wrap1, d: Wrap1, e: Wrap1)
}

enum Wrap3 {
    N,
    D(a: Wrap2, b: Wrap2, c: Wrap2, d: Wrap2, e: Wrap2)
}

enum Wrap4 {
    N,
    D(a: Wrap3, b: Wrap3, c: Wrap3, d: Wrap3, e: Wrap3)
}

enum Wrap5 {
    N,
    D(a: Wrap4, b: Wrap4, c: Wrap4, d: Wrap4, e: Wrap4)
}

enum Wrap6 {
    N,
    D(a: Wrap5, b: Wrap5, c: Wrap5, d: Wrap5, e: Wrap5)
}

atom struct_array_enum_equality_exceeds_budget(a: Wrap6, b: Wrap6) -> bool
requires: true;
ensures: result == true;
body: a == b;
