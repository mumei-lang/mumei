struct R {
    x: i64,
    r_x: i64,
    r: R
}

atom recursive_path_false_claim(s: R) -> i64
    requires: true;
    ensures: s.r.x == s.r_x;
    body: 0
