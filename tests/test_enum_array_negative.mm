enum C {
    R(v: i64),
    G
}

enum D {
    E
}

struct Payload {
    xs: [i64]
}

enum ArrayPayload {
    Data(xs: [i64]),
    Empty
}

enum StructArrayPayload {
    Data(p: Payload),
    Empty
}

enum Recursive {
    Next(value: Recursive),
    End
}

enum Generic<T> {
    Some(value: T),
    None
}

atom false_tag_claim(xs: [C]) -> bool
requires: len(xs) > 0;
ensures: match xs[0] {
    C::R(_) => false,
    C::G => true
};
body: true;

atom stale_same_index_read(xs: [C], i: i64) -> i64
requires: i >= 0 && i < len(xs) && match xs[i] {
    C::R(v) => v == 7,
    C::G => false
};
ensures: result == 7;
body: {
    xs[i] = C::R(5);
    match xs[i] {
        C::R(v) => v,
        C::G => 0
    }
};

atom mixed_enum_literal() -> i64
requires: true;
ensures: true;
body: len([C::G, D::E]);

atom mismatched_enum_store(xs: [C]) -> i64
requires: len(xs) > 0;
ensures: true;
body: {
    xs[0] = D::E;
    0
};

atom array_payload_enum(xs: [ArrayPayload]) -> i64
requires: true;
ensures: true;
body: len(xs);

atom struct_array_payload_enum(xs: [StructArrayPayload]) -> i64
requires: true;
ensures: true;
body: len(xs);

atom recursive_enum_array(xs: [Recursive]) -> i64
requires: true;
ensures: true;
body: len(xs);

atom unresolved_generic_enum_array(xs: [Generic]) -> i64
requires: true;
ensures: true;
body: len(xs);
