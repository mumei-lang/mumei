atom block_reassign() -> i64
requires: true;
ensures: true;
body: {
    let xs = { let t = [1, 2, 3]; t = [9]; t };
    xs[2]
};

atom alias_before_source_reassign() -> i64
requires: true;
ensures: true;
body: {
    let xs = { let source = [1]; let alias = source; source = [4, 5, 6]; alias };
    xs[2]
};

atom alias_of_outer_before_reassign() -> i64
requires: true;
ensures: true;
body: {
    let s = [1];
    let xs = { let alias = s; s = [4, 5, 6]; alias };
    xs[2]
};

atom nested_alias_of_outer_before_reassign() -> i64
requires: true;
ensures: true;
body: {
    let s = [1];
    let xs = { let alias = { let inner = s; inner }; s = [4, 5, 6]; alias };
    xs[2]
};

atom if_reassign(c: bool) -> i64
requires: true;
ensures: true;
body: {
    let xs = if c { let t = [1, 2, 3]; t = [9]; t } else { [1, 2, 3] };
    xs[2]
};
