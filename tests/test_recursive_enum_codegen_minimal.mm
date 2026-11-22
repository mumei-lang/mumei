enum L {
    Nil,
    Cons(h: i64, t: L)
}

atom recursive(l: L) -> i64
requires: true;
ensures: result == 0;
body: {
    0
};

atom main() -> i64
requires: true;
ensures: result == 0;
body: {
    0
};
