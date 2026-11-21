struct B {
    k: i64,
    values: [i64]
}

atom get1(b: B) -> i64
    requires: len(b.values) > 1;
    ensures: result == b.values[1];
    body: b.values[1]
