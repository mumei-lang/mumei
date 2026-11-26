// Generic struct declarations (`pure-i64-composite/v9`) — negative
// atoms: each postcondition contradicts what the body produces, so
// every atom must fail verification.

struct Pair<T> {
    a: T,
    b: T
}

atom bad_sum(p: Pair<i64>) -> i64
requires: true;
ensures: result == p.a + p.b + 1;
body: {
    p.a + p.b
};

atom bad_swap(p: Pair<i64>) -> Pair<i64>
requires: true;
ensures: result.a == p.a;
body: {
    Pair<i64>{a: p.b, b: p.a}
};

atom bad_mk(a: i64) -> Pair<i64>
requires: true;
ensures: result.b == a + 1;
body: {
    Pair<i64>{a: a, b: a}
};
