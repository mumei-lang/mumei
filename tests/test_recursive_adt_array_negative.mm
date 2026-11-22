enum L {
    Nil,
    Cons(head: i64, tail: L),
}

atom unsupported_array(values: [L]) -> i64
    requires: true;
    ensures: true;
    body: 0
