enum T {
    N,
    D(tag: [i64])
}

atom tag_of(xs: [i64]) -> i64
requires: len(xs) == 2;
ensures: result == 1;
body: {
    let e = T::D(xs);
    let t = e.tag;
    t
};

atom main() -> i64
requires: true;
ensures: result == 1;
body: {
    tag_of([10, 20])
};
