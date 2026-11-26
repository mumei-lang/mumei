atom invoke(f: atom_ref([i64]) -> i64, xs: [i64]) -> i64
requires: true;
ensures: true;
body: call(f, xs);

atom caller(xs: [i64]) -> i64
requires: len(xs) == 0;
ensures: true;
body: {
    let h = |ys: [i64]| ys[0];
    invoke(h, xs)
};
