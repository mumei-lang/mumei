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

atom block_alias_tail() -> i64
requires: true;
ensures: true;
body: {
    let s = [4, 5, 6];
    let xs = { let alias = s; alias };
    xs[2]
};

atom block_alias_then_source_reassign_tail() -> i64
requires: true;
ensures: true;
body: {
    let xs = { let source = [4, 5, 6]; let alias = source; source = [1]; alias };
    xs[2]
};

atom block_reassign_tail() -> i64
requires: true;
ensures: true;
body: {
    let xs = { let t = [1]; t = [4, 5, 6]; t };
    xs[2]
};
