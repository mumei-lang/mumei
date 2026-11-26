struct Inner {
    x: i64,
    y: i64
}

struct P {
    inner: Inner,
    z: i64
}

enum Ev {
    Move(p: P),
    Stop
}

enum OuterEv {
    Wrap(c: Ev, marker: i64),
    Nil
}

enum Positional {
    Move(P),
    Stop
}

enum Ambiguous {
    Left(p: P),
    Right(p: P),
    Stop
}

struct FlatP {
    x: i64
}

enum FlatEv {
    Move(p: FlatP),
    Stop
}

atom get_p(p: P) -> i64
requires: true;
ensures: result == p.inner.x;
body: p.inner.x;

atom bad_wrong_leaf(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let e = Ev::Move(P { inner: Inner { x: v, y: 7 }, z: 3 });
    let q = e.p;
    q.inner.y
};

atom bad_unnarrowed_parameter(e: Ev, v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let q = e.p;
    q.inner.x
};

atom bad_wrong_variant(e: Ev, v: i64) -> i64
requires: match e {
    Ev::Stop => true,
    _ => false
};
ensures: result == v;
body: {
    let q = e.p;
    q.inner.x
};

atom bad_wrong_outer_variant(o: OuterEv, v: i64) -> i64
requires: match o {
    OuterEv::Nil => true,
    _ => false
};
ensures: result == v;
body: {
    let q = o.c.p;
    q.inner.x
};

atom bad_nominal_mismatch(e: Ev) -> i64
requires: match e {
    Ev::Move(_) => true,
    _ => false
};
ensures: true;
body: get_p(e.p.inner);

atom bad_unknown_field(e: Ev, v: i64) -> i64
requires: match e {
    Ev::Move(_) => true,
    _ => false
};
ensures: result == v;
body: e.p.missing;

atom bad_positional_payload(e: Positional, v: i64) -> i64
requires: match e {
    Positional::Move(p) => p.inner.x == v,
    _ => false
};
ensures: result == v;
body: e.p.inner.x;

atom bad_ambiguous_owner(e: Ambiguous, v: i64) -> i64
requires: match e {
    Ambiguous::Left(p) => p.inner.x == v,
    _ => false
};
ensures: result == v;
body: e.p.inner.x;

atom bad_both_unproven_rebind(e: FlatEv, e2: FlatEv) -> i64
requires: true;
ensures: result == 0;
body: {
    let q = e.p;
    let a = q.x;
    let q = e2.p;
    q.x - a
};

atom bad_both_unproven_rebind_assign(e: FlatEv, e2: FlatEv) -> i64
requires: true;
ensures: result == 0;
body: {
    let q = e.p;
    let a = q.x;
    q = e2.p;
    q.x - a
};

atom bad_both_unproven_nested_rebind(e: Ev, e2: Ev) -> i64
requires: true;
ensures: result == 0;
body: {
    let q = e.p;
    let a = q.inner.x;
    let q = e2.p;
    q.inner.x - a
};
