enum C {
    R(v: i64),
    G
}

enum Unit {
    A,
    B
}

struct Box {
    xs: [C]
}

atom p(xs: [C]) -> i64
requires: len(xs) > 0 && match xs[0] {
    C::R(v) => v > 5,
    C::G => false
};
ensures: result > 5;
body: match xs[0] {
    C::R(v) => v,
    C::G => 0
};

atom store_then_read(xs: [C], i: i64) -> i64
requires: i >= 0 && i < len(xs);
ensures: result == 5;
body: {
    xs[i] = C::R(5);
    match xs[i] {
        C::R(v) => v,
        C::G => 0
    }
};

atom literal_round_trip() -> i64
requires: true;
ensures: result == 7;
body: {
    let xs = [C::R(5), C::G];
    let second = match xs[1] {
        C::R(_) => 10,
        C::G => 0
    };
    let first = match xs[0] {
        C::R(v) => v,
        C::G => 0
    };
    len(xs) + second + first
};

atom store_preserves_other_index(xs: [C]) -> i64
requires: len(xs) > 1 &&
    match xs[1] {
        C::R(old) => old == 7,
        C::G => false
    };
ensures: result == 7;
body: {
    xs[0] = C::G;
    match xs[1] {
        C::R(old) => old,
        C::G => 0
    }
};

atom unit_enum_array() -> i64
requires: true;
ensures: result == 2;
body: {
    let xs = [Unit::A, Unit::B];
    xs[1] = Unit::A;
    len(xs)
};

atom count_red() -> i64
requires: true;
ensures: result >= 0 && result <= 2;
body: {
    let xs = [C::R(1), C::G];
    let i = 0;
    let count = 0;
    while i < 2
    invariant: i >= 0 && i <= 2 && count >= 0 && count <= i
    decreases: 2 - i
    {
        let n = match xs[i] {
            C::R(_) => 1,
            C::G => 0
        };
        count = count + n;
        i = i + 1
    };
    count
};

atom extracted_enum_array(b: Box) -> i64
requires: len(b.xs) > 0;
ensures: true;
body: {
    let xs = b.xs;
    match xs[0] {
        C::R(v) => v,
        C::G => 0
    }
};

atom loop_store_enum_array(xs: [C]) -> i64
requires: len(xs) > 0;
ensures: true;
body: {
    let i = 0;
    while i < 1
    invariant: i >= 0 && i <= 1
    decreases: 1 - i
    {
        xs[i] = C::R(5);
        i = i + 1
    };
    0
};

atom loop_store_bool_array(xs: [bool]) -> i64
requires: len(xs) > 0;
ensures: true;
body: {
    let i = 0;
    while i < 1
    invariant: i >= 0 && i <= 1
    decreases: 1 - i
    {
        xs[i] = false;
        i = i + 1
    };
    0
};

atom loop_store_f64_array(xs: [f64], value: f64) -> i64
requires: len(xs) > 0;
ensures: true;
body: {
    let i = 0;
    while i < 1
    invariant: i >= 0 && i <= 1
    decreases: 1 - i
    {
        xs[i] = value;
        i = i + 1
    };
    0
};
