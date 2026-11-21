struct Point {
    x: i64
}

struct WrappedArray {
    values: [i64]
}

enum Ev {
    Data(xs: [i64]),
    Idle
}

enum NestedArray {
    Data(xs: [[i64]]),
    Idle
}

enum StructArray {
    Data(xs: [Point]),
    Idle
}

enum StructPayload {
    Data(value: WrappedArray),
    Idle
}

atom false_length_claim(xs: [i64]) -> i64
requires: len(xs) > 2;
ensures: result > 3;
body: {
    let e = Ev::Data(xs);
    match e {
        Ev::Data(ys) => len(ys),
        Ev::Idle => 0
    }
};

atom out_of_bounds_payload_read() -> i64
requires: true;
ensures: result >= 0;
body: {
    let e = Ev::Data([1, 2, 3]);
    match e {
        Ev::Data(ys) => ys[3],
        Ev::Idle => 0
    }
};

atom independent_payload_lengths(a: [i64], b: [i64]) -> bool
requires: len(a) == 2 && len(b) == 3;
ensures: result == true;
body: {
    let e1 = Ev::Data(a);
    let e2 = Ev::Data(b);
    match e1 {
        Ev::Data(xs) => match e2 {
            Ev::Data(ys) => len(xs) == len(ys),
            Ev::Idle => false
        },
        Ev::Idle => false
    }
};

atom different_direct_payload_roots_keep_lengths_separate(e1: Ev, e2: Ev) -> bool
requires:
    match e1 {
        Ev::Data(xs) => len(xs) == 2,
        Ev::Idle => false
    }
    &&
    match e2 {
        Ev::Data(ys) => len(ys) == 3,
        Ev::Idle => false
    };
ensures: result == true;
body: {
    let a = e1.xs;
    let b = e2.xs;
    len(a) == len(b)
};

atom array_inside_struct_payload(values: [i64]) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = StructPayload::Data(WrappedArray { values: values });
    0
};

atom nested_array_payload(xs: [[i64]]) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = NestedArray::Data(xs);
    0
};

atom struct_array_payload(xs: [Point]) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = StructArray::Data(xs);
    0
};

atom untagged_array_read(e: Ev) -> i64
requires: true;
ensures: result == 0;
body: len(e.xs);
