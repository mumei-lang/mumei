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

atom get_a(a: A) -> i64
requires: true;
ensures: result == a.x;
body: a.x;

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

atom unshadowed_payload_mismatch(e: Ev) -> i64
requires: true;
ensures: true;
body: get_a(e.f);
