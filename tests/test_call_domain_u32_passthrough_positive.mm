atom g(x: u32) -> i64
    requires: true;
    ensures: result == x;
    body: x

atom f(n: u32) -> i64
    requires: true;
    ensures: result == n;
    body: g(n)
