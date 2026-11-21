struct A {
    b_c: i64
}

struct B {
    c: i64
}

struct O {
    a: A,
    a_b: B
}

atom colliding_deep_field_tail(v: i64) -> i64
    requires: true;
    ensures: result == v;
    body: {
        let o = O { a: A { b_c: 1 }, a_b: B { c: 7 } };
        o.a.b_c = v
    }

atom bad_colliding_deep_field_returns_sibling(v: i64) -> i64
    requires: true;
    ensures: result == 7;
    body: {
        let o = O { a: A { b_c: 1 }, a_b: B { c: 7 } };
        o.a.b_c = v
    }
