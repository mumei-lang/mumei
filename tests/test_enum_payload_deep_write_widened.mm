struct Inner {
    x: i64,
    y: i64
}

struct Outer {
    inner: Inner,
    z: i64
}

struct Wider {
    a: i64,
    b: i64,
    c: i64,
    d: i64
}

enum Event {
    Pad(q: Wider),
    Move(p: Outer),
    Idle
}

atom deep_write_widened(e: Event, v: i64) -> i64
requires: match e {
    Event::Move(_) => true,
    _ => false
};
ensures: result == v;
body: e.p.inner.x = v;
