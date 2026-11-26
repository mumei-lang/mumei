atom g(xs: [u32]) -> i64
    requires: true;
    ensures: result == len(xs);
    body: len(xs);

atom f(xs: [u32]) -> i64
    requires: true;
    ensures: result == len(xs);
    body: {
        let h = |ys: [u32]| g(ys);
        h(xs)
    }
