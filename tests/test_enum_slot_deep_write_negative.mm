type NonNegative = i64 where v >= 0;

struct B {
    e: Chan,
    k: i64,
    invariant: self.k >= 0
}

struct ScalarBox {
    e: i64
}

struct ArrayBox {
    e: [Chan]
}

struct RecursiveBox {
    e: RecursiveChan
}

struct RefinedBox {
    e: RefinedChan
}

struct PositionalBox {
    e: PositionalChan
}

struct AmbiguousBox {
    e: AmbiguousChan
}

struct Inner {
    x: i64,
    y: i64
}

struct OtherInner {
    x: i64,
    y: i64
}

struct NominalBox {
    e: NominalChan,
    k: i64
}

enum Chan {
    Tcp(port: i64, keep: i64),
    Udp
}

enum OuterEnum {
    Wrap(c: Chan),
    Nil
}

enum RefinedChan {
    Tcp(port: NonNegative),
    Udp
}

enum PositionalChan {
    Tcp(i64),
    Udp
}

enum AmbiguousChan {
    Tcp(port: i64),
    Udp(port: i64)
}

enum RecursiveChan {
    Next(next: RecursiveChan),
    End
}

enum NominalChan {
    Tcp(p: Inner),
    Udp
}

atom bad_wrong_inner_tag() -> i64
requires: true;
ensures: true;
body: {
    let o = OuterEnum::Wrap(Chan::Udp);
    o.c.port = 1;
    0
}

atom bad_unknown_inner_tag(o: OuterEnum) -> i64
requires: true;
ensures: true;
body: match o {
    OuterEnum::Wrap(c) => {
        o.c.port = 1;
        0
    },
    _ => 0
};

atom bad_wrong_outer_tag() -> i64
requires: true;
ensures: true;
body: {
    let o = OuterEnum::Nil;
    o.c.port = 1;
    0
}

atom bad_unnarrowed_parameter(o: OuterEnum) -> i64
requires: true;
ensures: true;
body: {
    o.c.port = 1;
    0
}

atom bad_unknown_field(b: B) -> i64
requires: true;
ensures: true;
body: {
    b.e.missing = 1;
    0
}

atom bad_positional_inner_payload(b: PositionalBox) -> i64
requires: true;
ensures: true;
body: {
    b.e.port = 1;
    0
}

atom bad_ambiguous_inner_owner(b: AmbiguousBox) -> i64
requires: true;
ensures: true;
body: {
    b.e.port = 1;
    0
}

atom bad_scalar_instead_of_enum(b: ScalarBox) -> i64
requires: true;
ensures: true;
body: {
    b.e.port = 1;
    0
}

atom bad_array_path(b: ArrayBox) -> i64
requires: true;
ensures: true;
body: {
    b.e.port = 1;
    0
}

atom bad_recursive_enum_path(b: RecursiveBox) -> i64
requires: true;
ensures: true;
body: {
    b.e.next.next = 1;
    0
}

atom bad_refined_inner_payload() -> i64
requires: true;
ensures: true;
body: {
    let b = RefinedBox { e: RefinedChan::Tcp(0) };
    b.e.port = -1;
    0
}

atom bad_slot_rhs_double_eval() -> i64
requires: true;
ensures: result == 4;
body: {
    let w = 0;
    let b = B { e: Chan::Tcp(1, 9), k: 7 };
    b.e.port = { w = w + 1; w };
    w + w
}

atom bad_slot_rebinds_base() -> i64
requires: true;
ensures: true;
body: {
    let b = B { e: Chan::Tcp(1, 9), k: 7 };
    b.e.port = {
        b = B { e: Chan::Tcp(3, 4), k: 5 };
        1
    };
    0
}

atom bad_slot_false_ensures(v: i64) -> i64
requires: true;
ensures: result == v + 1;
body: {
    let b = B { e: Chan::Tcp(1, 9), k: 7 };
    b.e.port = v
}

atom bad_slot_direct_read_false_ensures(b: B, v: i64) -> i64
requires: match b.e {
    Chan::Tcp(_, _) => true,
    _ => false
};
ensures: result == v + 1;
body: match b.e {
    Chan::Tcp(_, _) => {
        b.e.port = v;
        b.e.port
    },
    _ => 0
};

atom bad_outer_slot_direct_read_false_ensures(o: OuterEnum, v: i64) -> i64
requires: match o {
    OuterEnum::Wrap(c) => match c {
        Chan::Tcp(_, _) => true,
        _ => false
    },
    _ => false
};
ensures: result == v + 1;
body: {
    o.c.port = v;
    o.c.port
}

atom bad_nominal_enum_slot_write() -> i64
requires: true;
ensures: true;
body: {
    let b = NominalBox {
        e: NominalChan::Tcp(Inner { x: 1, y: 2 }),
        k: 3
    };
    b.e.p = OtherInner { x: 4, y: 5 };
    0
}
