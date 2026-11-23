atom make3(n: i64) -> [i64]
requires: n >= 0;
ensures: len(result) == 3;
effects: [];
body: {
    [1, 2, 3]
};

atom make4(n: i64) -> [i64]
requires: n >= 0;
ensures: len(result) == 4;
effects: [];
body: {
    [1, 2, 3, 4]
};

atom passthrough(a: [i64]) -> [i64]
requires: len(a) >= 0;
ensures: len(result) == len(a);
effects: [];
body: {
    a
};

atom relit(n: i64) -> [i64]
requires: n >= 0;
ensures: len(result) == 3;
effects: [];
body: {
    let t = make3(n);
    passthrough(t)
};

atom pick(c: bool, a: [i64]) -> [i64]
requires: len(a) >= 0 && (c == true || c == false);
ensures: len(result) == 3 || len(result) == len(a);
effects: [];
body: {
    if c { make3(0) } else { a }
};

atom main() -> i64
effects: [];
body: {
    let t = make3(0);
    let s = relit(0);
    t = make4(0);
    let p = pick(true, t);
    t[0] + s[0] + len(t) + len(p) + t[3]
};
