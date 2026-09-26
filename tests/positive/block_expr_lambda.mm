atom f(x: i64) -> i64
requires: true;
ensures: result == x + 1;
body: {
    let g = { let h = |n: i64| -> i64 { n + 1 }; h };
    g(x)
};
