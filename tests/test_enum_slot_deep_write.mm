struct Inner {
    x: i64,
    y: i64
}

struct ChannelHolder {
    ch: Chan,
    marker: i64
}

struct B {
    e: Chan,
    k: i64,
    invariant: self.k >= 0
}

struct Cell<T> {
    value: T
}

struct Holder {
    cell: Cell<u64>
}

enum Chan {
    Tcp(port: i64, keep: i64),
    Udp
}

enum OuterEnum {
    Wrap(c: Chan, marker: i64),
    Nil
}

enum MixedEnum {
    Wrap(p: ChannelHolder),
    Nil
}

atom local_outer_slot(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let o = OuterEnum::Wrap(Chan::Tcp(1, 9), 5);
    o.c.port = v
}

atom local_struct_slot(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let b = B { e: Chan::Tcp(1, 9), k: 7 };
    b.e.port = v
}

atom outer_match_arm_slot(o: OuterEnum, v: i64) -> i64
requires: true;
ensures: true;
body: match o {
    OuterEnum::Wrap(c, _) => match c {
        Chan::Tcp(_, _) => {
            o.c.port = v;
            0
        },
        _ => 0
    },
    _ => 0
};

atom struct_parameter_match_arm_slot(b: B, v: i64) -> i64
requires: match b.e {
    Chan::Tcp(_, _) => true,
    _ => false
};
ensures: result == v;
body: match b.e {
    Chan::Tcp(_, _) => {
        b.e.port = v;
        b.e.port
    },
    _ => 0
};

atom outer_parameter_direct_slot_read(o: OuterEnum, v: i64) -> i64
requires: match o {
    OuterEnum::Wrap(c, _) => match c {
        Chan::Tcp(_, _) => true,
        _ => false
    },
    _ => false
};
ensures: true;
body: {
    o.c.port = v;
    o.c.port
}

atom reread_outer_slot_preserves_siblings(v: i64) -> i64
requires: true;
ensures: result == 1;
body: {
    let o = OuterEnum::Wrap(Chan::Tcp(1, 9), 5);
    o.c.port = v;
    match o {
        OuterEnum::Wrap(c, marker) => match c {
            Chan::Tcp(port, keep) =>
                if port == v && keep == 9 && marker == 5 { 1 } else { 0 },
            _ => 0
        },
        _ => 0
    }
}

atom reread_struct_slot_preserves_siblings(v: i64) -> i64
requires: true;
ensures: result == 1;
body: {
    let b = B { e: Chan::Tcp(1, 9), k: 7 };
    b.e.port = v;
    match b.e {
        Chan::Tcp(port, keep) =>
            if port == v && keep == 9 && b.k == 7 { 1 } else { 0 },
        _ => 0
    }
}

atom mixed_enum_struct_enum_slot(v: i64) -> i64
requires: true;
ensures: result == v;
body: {
    let e = MixedEnum::Wrap(ChannelHolder {
        ch: Chan::Tcp(1, 9),
        marker: 6
    });
    e.p.ch.port = v
}

atom struct_slot_rhs_once() -> i64
requires: true;
ensures: result == 2;
body: {
    let w = 0;
    let b = B { e: Chan::Tcp(1, 9), k: 7 };
    b.e.port = { w = w + 1; w };
    w + w
}

atom enum_slot_ir(o: OuterEnum, v: i64) -> i64
requires: match o {
    OuterEnum::Wrap(c, _) => match c {
        Chan::Tcp(_, _) => true,
        _ => false
    },
    _ => false
};
ensures: true;
body: o.c.port = v;

atom struct_slot_ir(b: B, v: i64) -> i64
requires: match b.e {
    Chan::Tcp(_, _) => true,
    _ => false
};
ensures: true;
body: b.e.port = v;

atom generic_holder_unsigned_ops(h: Holder) -> u64
requires: true;
ensures: true;
body: {
    h.cell.value > 0;
    h.cell.value / 2
}
