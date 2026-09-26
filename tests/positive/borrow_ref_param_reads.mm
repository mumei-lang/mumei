atom first(ref x: [i64]) -> i64
requires: len(x) >= 1;
ensures: true;
body: {
    let n = len(x);
    let v = x[0];
    v + n
};

atom sum2(ref mut x: [i64]) -> i64
requires: len(x) >= 2;
ensures: true;
body: {
    x[0] = 1;
    x[0] + x[1]
};
