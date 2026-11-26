enum C {
    R,
    G(v: u32)
}

struct P {
    x: i64,
    c: C
}

atom match_enum_leaf(ps: [P]) -> bool
requires: len(ps) > 0 && ps[0].c == C::G(7);
ensures: result == true;
body: match ps[0].c {
    C::R => false,
    C::G(v) => v == 7
};

atom match_bound_element(ps: [P]) -> bool
requires: len(ps) > 0 && ps[0].c == C::G(9);
ensures: result == true;
body: {
    let p = ps[0];
    match p.c {
        C::R => false,
        C::G(v) => v == 9
    }
};

atom write_enum_leaf(ps: [P]) -> bool
requires: len(ps) > 0;
ensures: result == true;
body: {
    ps[0].c = C::G(11);
    match ps[0].c {
        C::R => false,
        C::G(v) => v == 11
    }
};

atom write_struct_element(ps: [P]) -> bool
requires: len(ps) > 0;
ensures: result == true;
body: {
    ps[0] = P { x: 3, c: C::G(13) };
    let p = ps[0];
    match p.c {
        C::R => false,
        C::G(v) => v == 13
    }
};

atom enum_leaf_callee(ps: [P]) -> bool
requires: len(ps) > 0 && ps[0].c == C::G(3);
ensures: result == true;
body: true;

atom pass_enum_leaf_array(ps: [P]) -> bool
requires: len(ps) > 0 && ps[0].c == C::G(3);
ensures: result == true;
body: enum_leaf_callee(ps);

atom enum_leaf_forall(ps: [P]) -> bool
requires: true;
ensures: forall(i, 0, len(ps), match ps[i].c {
    C::R => true,
    C::G(v) => v <= 4294967295
});
body: true;

atom enum_payload_domain(ps: [P]) -> bool
requires: len(ps) > 0;
ensures: result == true;
body: match ps[0].c {
    C::R => true,
    C::G(v) => v >= 0 && v <= 4294967295
};
