enum C {
    R,
    G(v: u32)
}

struct P {
    x: i64,
    c: C
}

atom stale_enum_leaf_read(ps: [P]) -> bool
requires: len(ps) > 0;
ensures: result == true;
body: {
    ps[0].c = C::R;
    match ps[0].c {
        C::R => false,
        C::G(_) => true
    }
};

atom take_enum_leaf_array(ps: [P]) -> bool
requires: len(ps) > 0;
ensures: result == true;
body: true;

atom enum_leaf_call_overflow(v: u64) -> bool
requires: true;
ensures: result == true;
body: take_enum_leaf_array([P { x: 0, c: C::G(v) }]);

atom claim_enum_payload_overflow(ps: [P]) -> bool
requires: len(ps) > 0;
ensures: result == true;
body: match ps[0].c {
    C::R => false,
    C::G(v) => v > 4294967295
};
