struct S {
    xs: [i64]
}

enum E {
    D(s: S),
    N
}

atom build_array_payload(s: S) -> i64
requires: true;
ensures: result == 0;
body: {
    let e = E::D(s);
    match e {
        E::D(_) => 0,
        E::N => 0
    }
};
