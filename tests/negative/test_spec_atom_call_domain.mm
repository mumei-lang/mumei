atom g(x: u32) -> i64
    requires: true;
    ensures: result >= 0;
    body: 0;

atom f(n: i64) -> i64
    requires: true;
    ensures: g(n) >= 0;
    body: 0;
