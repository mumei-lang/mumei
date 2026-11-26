struct Inner {
    x: i64,
    y: i64
}

struct Outer {
    inner: Inner,
    z: i64
}

struct InBox {
    inner: Inner,
    invariant: self.inner.x >= 0
}

struct Holder {
    a: Outer
}

atom ordinary_deep_rhs_once() -> i64
requires: true;
ensures: result == 2;
body: {
    let w = 0;
    let o = Outer { inner: Inner { x: 1, y: 2 }, z: 3 };
    o.inner.x = { w = w + 1; w };
    w + w
}

atom ordinary_struct_rhs_write(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let o = Holder { a: Outer { inner: Inner { x: 1, y: 2 }, z: 3 } };
    o.a.inner = Inner { x: v, y: v };
    o.a.inner.x
}

enum Event {
    Move(p: Outer),
    Idle
}

enum GuardedEvent {
    Store(p: InBox),
    Empty
}

atom deep_write(e: Event, v: i64) -> i64
requires: match e {
    Event::Move(_) => true,
    _ => false
};
ensures: result == v;
body: e.p.inner.x = v;

atom enum_deep_rhs_once() -> i64
requires: true;
ensures: result == 2;
body: {
    let w = 0;
    let e = Event::Move(Outer { inner: Inner { x: 1, y: 2 }, z: 3 });
    e.p.inner.x = { w = w + 1; w };
    w + w
}

atom shallow_write(e: Event, v: i64) -> i64
requires: match e {
    Event::Move(_) => true,
    _ => false
};
ensures: result == v;
body: e.p.z = v;

atom replace_inner(e: Event, v: i64) -> i64
requires: match e {
    Event::Move(_) => true,
    _ => false
};
ensures: result == v;
body: {
    e.p.inner = Inner { x: v, y: v };
    v
}

atom write_in_match_arm(e: Event, v: i64) -> i64
requires: true;
ensures: true;
body: match e {
    Event::Move(_) => {
        e.p.inner.x = v;
        0
    },
    _ => 0
};

atom preserves_siblings(v: i64) -> i64
requires: true;
ensures: result == 1;
body: {
    let e = Event::Move(Outer {
        inner: Inner { x: 1, y: 2 },
        z: 3
    });
    e.p.inner.x = v;
    match e {
        Event::Move(p) => if p.inner.x == v && p.inner.y == 2 && p.z == 3 { 1 } else { 0 },
        _ => 0
    }
}

atom satisfies_invariant(v: i64) -> i64
requires: v >= 0;
ensures: result == v;
body: {
    let e = GuardedEvent::Store(InBox {
        inner: Inner { x: 1, y: 2 }
    });
    e.p.inner.x = v
}
