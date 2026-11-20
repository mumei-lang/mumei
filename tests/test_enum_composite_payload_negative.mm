// Rejected composite enum payloads.

struct Point {
    x: i64,
    y: i64
}

enum Event {
    Ping,
    Move(pos: Point)
}

enum Dual {
    A(pos: Point),
    B(pos: Point)
}

// Scalar arg for a composite payload — the constructor's slot kind is
// `Point`, not i64.
atom bad_scalar_ctor(v: i64) -> Event
requires: true;
ensures: true;
body: {
    Event::Move(v)
};

// A scalar write into a composite slot — `pos` holds a `Point`.
atom bad_scalar_write(e: Event, v: i64) -> Event
requires: true;
ensures: true;
body: {
    e.pos = v;
    e
};

// Wrong tag: `e` holds `Ping`, so `is-Move(e)` is refutable.
atom bad_wrong_tag(p: Point) -> Event
requires: true;
ensures: true;
body: {
    let e = Event::Ping;
    e.pos = p;
    e
};

// Unknown tag: a param outside match narrowing carries no proof.
atom bad_unknown_tag(e: Event, p: Point) -> Event
requires: true;
ensures: true;
body: {
    e.pos = p;
    e
};

// Ambiguous payload name: `pos` is a slot of both `A` and `B`.
atom bad_ambiguous(e: Dual, p: Point) -> Dual
requires: true;
ensures: true;
body: {
    match e {
        Dual::A(pos) => { e.pos = p; e }
        _ => e
    }
};
