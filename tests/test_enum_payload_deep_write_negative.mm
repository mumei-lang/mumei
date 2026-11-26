type NonNegative = i64 where v >= 0;

struct Inner {
    x: i64,
    y: i64
}

struct OtherInner {
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

struct RefinedPoint {
    x: NonNegative,
    y: i64
}

enum Event {
    Move(p: Outer),
    Idle
}

enum GuardedEvent {
    Store(p: InBox),
    Empty
}

enum RefinedEvent {
    Update(p: RefinedPoint),
    Empty
}

enum ScalarEvent {
    Scalar(value: i64),
    Empty
}

enum PositionalEvent {
    Wrap(Outer),
    Empty
}

enum AmbiguousEvent {
    First(payload: Outer),
    Second(payload: Outer),
    Empty
}

atom bad_wrong_tag() -> i64
requires: true;
ensures: true;
body: {
    let e = Event::Idle;
    e.p.inner.x = 1
};

atom bad_unknown_tag(e: Event, v: i64) -> i64
requires: true;
ensures: true;
body: e.p.inner.x = v;

atom bad_nested_invariant() -> i64
requires: true;
ensures: true;
body: {
    let e = GuardedEvent::Store(InBox {
        inner: Inner { x: 0, y: 1 }
    });
    e.p.inner.x = 0 - 1
};

atom bad_refined_alias() -> i64
requires: true;
ensures: true;
body: {
    let e = RefinedEvent::Update(RefinedPoint { x: 0, y: 1 });
    e.p.x = 0 - 1
};

atom bad_unknown_field(v: i64) -> i64
requires: true;
ensures: true;
body: {
    let e = Event::Move(Outer {
        inner: Inner { x: 0, y: 1 },
        z: 2
    });
    e.p.inner.missing = v
};

atom bad_scalar_payload() -> i64
requires: true;
ensures: true;
body: {
    let e = ScalarEvent::Scalar(0);
    e.value.x = 1
};

atom bad_positional_payload() -> i64
requires: true;
ensures: true;
body: {
    let e = PositionalEvent::Wrap(Outer {
        inner: Inner { x: 0, y: 1 },
        z: 2
    });
    e.Outer.inner.x = 1
};

atom bad_ambiguous_owner(e: AmbiguousEvent, v: i64) -> i64
requires: true;
ensures: true;
body: e.payload.inner.x = v;

atom bad_false_ensures(e: Event, v: i64) -> i64
requires: true;
ensures: result == 1;
body: match e {
    Event::Move(_) => {
        e.p.z = v;
        0
    },
    _ => 0
};

atom bad_struct_rhs_double_eval() -> i64
requires: true;
ensures: result == 4;
body: {
    let w = 0;
    let o = Outer { inner: Inner { x: 1, y: 2 }, z: 3 };
    o.inner.x = { w = w + 1; w };
    w + w
};

atom bad_enum_rhs_double_eval() -> i64
requires: true;
ensures: result == 4;
body: {
    let w = 0;
    let e = Event::Move(Outer { inner: Inner { x: 1, y: 2 }, z: 3 });
    e.p.inner.x = { w = w + 1; w };
    w + w
};

atom bad_nominal_deep_payload_write() -> i64
requires: true;
ensures: true;
body: {
    let e = Event::Move(Outer {
        inner: Inner { x: 1, y: 2 },
        z: 3
    });
    e.p.inner = OtherInner { x: 1, y: 2 };
    0
};

atom bad_reread_sibling_relation() -> i64
requires: true;
ensures: result == 1;
body: {
    let v = 0;
    let e = Event::Move(Outer {
        inner: Inner { x: 1, y: 2 },
        z: 3
    });
    e.p.inner.x = v;
    match e {
        Event::Move(p) => if p.inner.y == v + 1 { 1 } else { 0 },
        _ => 0
    }
};

atom bad_deep_write_rebinds_base() -> i64
requires: true;
ensures: true;
body: {
    let o = Outer { inner: Inner { x: 1, y: 2 }, z: 3 };
    o.inner.x = { o = Outer { inner: Inner { x: 4, y: 5 }, z: 6 }; 1 };
    0
};
