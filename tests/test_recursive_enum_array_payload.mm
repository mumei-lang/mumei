enum E {
    Nil,
    V(xs: [E])
}

atom read(value: E) -> i64
requires: true;
ensures: result == 0;
body: {
    0
};
