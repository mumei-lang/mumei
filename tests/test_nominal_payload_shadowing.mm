struct A {
    x: i64
}

struct B {
    f: A
}

enum Ev {
    Wrap(f: B),
    Stop
}

struct P {
    x: i64
}

type PA = P;

struct Q {
    x: i64
}

enum ShadowPayload {
    Move(p: PA),
    Stop
}

enum InnerShadow {
    Leaf(p: PA),
    Empty
}

enum OuterShadow {
    Wrap(inner: InnerShadow),
    Stop
}

atom get_a(a: A) -> i64
requires: true;
ensures: result == a.x;
body: a.x;

atom take_q(q: Q) -> i64
requires: true;
ensures: result == q.x;
body: q.x;

atom shadowed_match_binding(e: Ev) -> i64
requires: match e {
    Ev::Wrap(_) => true,
    _ => false
};
ensures: true;
body: match e {
    Ev::Wrap(e) => get_a(e.f),
    _ => 0
};

atom shadowed_wrong_struct_type(e: ShadowPayload, p: Q) -> i64
requires: true;
ensures: true;
body: match e {
    ShadowPayload::Move(p) => take_q(p),
    _ => 0
};

atom nested_pattern_shadow_wrong_type(e: OuterShadow, p: Q) -> i64
requires: true;
ensures: true;
body: match e {
    OuterShadow::Wrap(InnerShadow::Leaf(p)) => take_q(p),
    _ => 0
};

atom unshadowed_payload_mismatch(e: Ev) -> i64
requires: true;
ensures: true;
body: get_a(e.f);
