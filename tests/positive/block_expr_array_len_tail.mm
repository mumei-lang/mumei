atom mk(n: i64) -> [i64]
requires: true;
ensures: len(result) == 2;
body: { [n, n] };

atom block_if_tail(c: bool) -> i64
requires: true;
ensures: result >= 0;
body: {
    let xs = { let k = 7; if c { [k, k] } else { [k, k, k] } };
    xs[1]
};

atom block_call_tail(n: i64) -> i64
requires: true;
ensures: true;
body: {
    let ys = { let m = n; mk(m) };
    ys[1]
};
