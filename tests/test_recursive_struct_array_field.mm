struct S {
    values: [S]
}

atom read(value: S) -> i64
requires: true;
ensures: result == 0;
body: {
    0
};
