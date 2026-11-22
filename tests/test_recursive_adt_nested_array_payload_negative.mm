struct NestedArray {
    xs: [[i64]],
}

enum L {
    Nil,
    Cons(payload: NestedArray, tail: L),
}

atom nested_array_payload_is_not_finite(payload: NestedArray) -> L
    requires: true;
    ensures: true;
    body: L::Cons(payload, L::Nil)
