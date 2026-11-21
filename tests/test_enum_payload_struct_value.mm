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

struct ShiftedP {
    marker: i64,
    inner: Inner,
    z: i64
}

enum ShiftedEv {
    Move(p: ShiftedP),
    Stop
}

enum OuterEv {
    Wrap(c: Ev, marker: i64),
    Nil
}

struct SafeP {
    x: i64,
    y: i64,
    invariant: self.x <= self.y
}

enum SafeEv {
    Store(p: SafeP),
    Stop
}

atom get_x(p: P) -> i64
requires: true;
ensures: result == p.inner.x;
body: p.inner.x;

atom get_safe_x(p: SafeP) -> i64
requires: p.x <= p.y;
ensures: result == p.x;
body: p.x;

atom whole_payload_let(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let e = Ev::Move(P { inner: Inner { x: v, y: 2 }, z: 3 });
    let q = e.p;
    q.inner.x
};

atom whole_inner_let(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let e = Ev::Move(P { inner: Inner { x: v, y: 2 }, z: 3 });
    let i = e.p.inner;
    i.x
};

atom nested_payload_subrange_after_sibling(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let e = ShiftedEv::Move(
        ShiftedP { marker: 1, inner: Inner { x: v, y: 2 }, z: 3 }
    );
    let i = e.p.inner;
    i.x
};

atom payload_param_pass(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let e = Ev::Move(P { inner: Inner { x: v, y: 2 }, z: 3 });
    get_x(e.p)
};

atom requires_narrowed_struct_value(e: Ev, v: i64) -> i64
requires: match e {
    Ev::Move(p) => p.inner.x == v,
    _ => false
};
ensures: result == v;
body: {
    let q = e.p;
    q.inner.x
};

atom nested_enum_struct_payload_value(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let o = OuterEv::Wrap(
        Ev::Move(P { inner: Inner { x: v, y: 2 }, z: 3 }),
        9
    );
    let q = o.c.p;
    q.inner.x
};

atom match_arm_struct_value(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let e = Ev::Move(P { inner: Inner { x: v, y: 2 }, z: 3 });
    match e {
        Ev::Move(_) => {
            let q = e.p;
            q.inner.x
        },
        _ => 0
    }
};

atom read_after_write_preserves_siblings(v: i64) -> i64
requires: true;
ensures: result == v + 5;
body: {
    let e = Ev::Move(P { inner: Inner { x: 1, y: 2 }, z: 3 });
    e.p.inner.x = v;
    let q = e.p;
    q.inner.x + q.inner.y + q.z
};

atom get_safe_x_from_payload() -> i64
requires: true;
ensures: result == 1;
body: {
    let e = SafeEv::Store(SafeP { x: 1, y: 2 });
    get_safe_x(e.p)
};

atom tag_proven_payload_rebind(e: Ev, e2: Ev, v1: i64, v2: i64) -> i64
requires: match e {
    Ev::Move(p) => p.inner.x == v1,
    _ => false
} && match e2 {
    Ev::Move(p) => p.inner.x == v2,
    _ => false
};
ensures: result == v2;
body: {
    let q = e.p;
    let old = q.inner.x;
    let q = e2.p;
    q.inner.x
};

atom tag_proven_payload_assign_rebind(e: Ev, e2: Ev, v1: i64, v2: i64) -> i64
requires: match e {
    Ev::Move(p) => p.inner.x == v1,
    _ => false
} && match e2 {
    Ev::Move(p) => p.inner.x == v2,
    _ => false
};
ensures: result == v2;
body: {
    let q = e.p;
    let old = q.inner.x;
    q = e2.p;
    q.inner.x
};

atom plain_struct_variable_rebind(p1: P, p2: P, v: i64) -> i64
requires: p2.inner.x == v;
ensures: result == v;
body: {
    let q = p1;
    let old = q.inner.x;
    let q = p2;
    q.inner.x
};

atom struct_init_rebind(v1: i64, v2: i64) -> i64
requires: true;
ensures: result == v2;
body: {
    let q = P { inner: Inner { x: v1, y: 2 }, z: 3 };
    let old = q.inner.x;
    let q = P { inner: Inner { x: v2, y: 4 }, z: 5 };
    q.inner.x
};
