struct Pair {
    x: i64,
    y: i64
}

struct Nested {
    inner: Pair,
    z: i64
}

struct B {
    e: Chan,
    k: i64,
    invariant: self.k >= 0
}

enum Chan {
    Tcp(port: i64, keep: i64),
    Udp
}

enum Outer {
    Wrap(c: Chan, marker: i64),
    Nil
}

enum Boxed {
    Hold(p: Pair),
    Empty
}

enum Deep {
    Move(p: Nested),
    Idle
}

atom read_local_port(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let e = Chan::Tcp(v, 9);
    e.port
};

atom read_parameter_port(e: Chan, v: i64) -> i64
requires: match e {
    Chan::Tcp(port, _) => port == v,
    _ => false
};
ensures: result == v;
body: e.port;

atom read_port_constrained_by_requires(e: Chan, v: i64) -> i64
requires: e.tag == 0 && e.port == v;
ensures: result == v;
body: e.port;

atom read_match_arm_port(e: Chan, v: i64) -> i64
requires: match e {
    Chan::Tcp(port, _) => port == v,
    _ => false
};
ensures: result == v;
body: match e {
    Chan::Tcp(_, _) => e.port,
    _ => 0
};

atom read_struct_enum_port(b: B, v: i64) -> i64
requires: match b.e {
    Chan::Tcp(port, _) => port == v,
    _ => false
};
ensures: result == v;
body: b.e.port;

atom read_outer_enum_port(o: Outer, v: i64) -> i64
requires: match o {
    Outer::Wrap(c, _) => match c {
        Chan::Tcp(port, _) => port == v,
        _ => false
    },
    _ => false
};
ensures: result == v;
body: o.c.port;

atom read_enum_struct_leaf(e: Boxed, v: i64) -> i64
requires: match e {
    Boxed::Hold(p) => p.x == v,
    _ => false
};
ensures: result == v;
body: e.p.x;

atom read_enum_nested_struct_leaf(e: Deep, v: i64) -> i64
requires: match e {
    Deep::Move(p) => p.inner.x == v,
    _ => false
};
ensures: result == v;
body: e.p.inner.x;

atom reread_struct_enum_port(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let b = B { e: Chan::Tcp(1, 9), k: 7 };
    b.e.port = v;
    b.e.port
};

atom reread_outer_enum_port(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let o = Outer::Wrap(Chan::Tcp(1, 9), 5);
    o.c.port = v;
    o.c.port
};

atom reread_enum_nested_struct_leaf(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let e = Deep::Move(Nested { inner: Pair { x: 1, y: 2 }, z: 3 });
    e.p.inner.x = v;
    e.p.inner.x
};
