atom f(x: i64) -> i64
requires: true;
ensures: result == x + 1;
body: {
    let g = { let h = |n: i64| -> i64 { n + 1 }; h };
    g(x)
};

atom pick(c: bool) -> i64
requires: true;
ensures: true;
body: {
    let inc = |n: i64| -> i64 { n + 1 };
    let m = if c { let k = inc; k } else { let d = |n: i64| -> i64 { n * 2 }; d };
    m(10)
};

atom cond_local() -> i64
requires: true;
ensures: result == 11;
body: {
    let a = |n: i64| -> i64 { n + 1 };
    let m = { let h = |n: i64| -> i64 { n * 2 }; if h(1) == 2 { a } else { h } };
    m(10)
};

atom main() -> i64
requires: true;
ensures: true;
body: { f(7) + pick(true) + pick(false) + cond_local() };
