enum Ev {
    Data(xs: [i64]),
    Idle
}

enum WordArray {
    Data(xs: [u64]),
    Idle
}

enum LengthChoice {
    Short,
    AlsoShort
}

atom round_trip_length(xs: [i64]) -> i64
requires: len(xs) > 2;
ensures: result > 2;
body: {
    let e = Ev::Data(xs);
    match e {
        Ev::Data(ys) => len(ys),
        Ev::Idle => 0
    }
};

atom in_bounds_payload_read(xs: [i64]) -> i64
requires: len(xs) > 2;
ensures: result == xs[1];
body: {
    let e = Ev::Data(xs);
    match e {
        Ev::Data(ys) => ys[1],
        Ev::Idle => 0
    }
};

atom payload_element_fact(xs: [i64]) -> i64
requires: len(xs) > 0 && xs[0] == 7;
ensures: result == 7;
body: {
    let e = Ev::Data(xs);
    match e {
        Ev::Data(ys) => ys[0],
        Ev::Idle => 0
    }
};

atom matched_payload_length_is_nonnegative(e: Ev) -> i64
requires: true;
ensures: result >= 0;
body: match e {
    Ev::Data(ys) => len(ys),
    Ev::Idle => 0
};

atom array_literal_round_trip() -> i64
requires: true;
ensures: result == 3;
body: {
    let e = Ev::Data([1, 2, 3]);
    match e {
        Ev::Data(ys) => len(ys),
        Ev::Idle => 0
    }
};

atom if_expression_ctor_length_is_known(c: bool) -> i64
requires: true;
ensures: result == 2;
body: {
    let e = Ev::Data(if c { [1, 2] } else { [3, 4] });
    match e {
        Ev::Data(ys) => len(ys),
        Ev::Idle => 0
    }
};

atom block_tail_ctor_length_is_known() -> i64
requires: true;
ensures: result == 3;
body: {
    let e = Ev::Data({ let ys = [1, 2, 3]; ys });
    match e {
        Ev::Data(xs) => len(xs),
        Ev::Idle => 0
    }
};

atom match_expression_ctor_length_is_known(c: LengthChoice) -> i64
requires: true;
ensures: result == 2;
body: {
    let e = Ev::Data(match c {
        LengthChoice::Short => [1, 2],
        LengthChoice::AlsoShort => [3, 4]
    });
    match e {
        Ev::Data(xs) => len(xs),
        Ev::Idle => 0
    }
};

atom match_binding_restores_outer_array(ys: [i64], xs: [i64]) -> i64
requires: len(ys) == 5 && ys[0] == 9 && len(xs) == 3;
ensures: result == 14;
body: {
    let e = Ev::Data(xs);
    let ignored = match e {
        Ev::Data(ys) => len(ys),
        Ev::Idle => 0
    };
    len(ys) + ys[0]
};

atom tagged_direct_array_read(e: Ev, v: i64) -> i64
requires: match e {
    Ev::Data(xs) => len(xs) > 1 && xs[1] == v,
    Ev::Idle => false
};
ensures: result == v;
body: {
    let ys = e.xs;
    ys[1]
};

atom tagged_direct_array_length(e: Ev) -> i64
requires: match e {
    Ev::Data(xs) => len(xs) == 4,
    Ev::Idle => false
};
ensures: result == 4;
body: len(e.xs);

atom tagged_direct_array_length_is_nonnegative(e: Ev) -> i64
requires: match e {
    Ev::Data(_) => true,
    Ev::Idle => false
};
ensures: result >= 0;
body: {
    let xs = e.xs;
    len(xs)
};

atom repeated_direct_payload_reads_keep_length(e: Ev) -> bool
requires: match e {
    Ev::Data(xs) => len(xs) == 4,
    Ev::Idle => false
};
ensures: result == true;
body: {
    let a = e.xs;
    let b = e.xs;
    len(a) == len(b)
};

atom tagged_array_write_then_read(e: Ev, ys: [i64], v: i64) -> i64
requires: match e {
    Ev::Data(_) => len(ys) > 1 && ys[1] == v,
    Ev::Idle => false
};
ensures: result == v;
body: {
    e.xs = ys;
    let xs = e.xs;
    xs[1]
};

atom u64_array_payload(xs: [u64]) -> u64
requires: len(xs) > 0 && xs[0] == 7;
ensures: result == 7;
body: {
    let e = WordArray::Data(xs);
    match e {
        WordArray::Data(ys) => ys[0],
        WordArray::Idle => 0
    }
};

atom enum_equality_implies_equal_payload_lengths(a: [i64], b: [i64]) -> bool
requires: true;
ensures: result == false || len(a) == len(b);
body: {
    let e1 = Ev::Data(a);
    let e2 = Ev::Data(b);
    e1 == e2
};
