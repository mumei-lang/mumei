atom block_reassign() -> i64
requires: true;
ensures: true;
body: {
    let xs = { let t = [1, 2, 3]; t = [9]; t };
    xs[2]
};

atom if_reassign(c: bool) -> i64
requires: true;
ensures: true;
body: {
    let xs = if c { let t = [1, 2, 3]; t = [9]; t } else { [1, 2, 3] };
    xs[2]
};
