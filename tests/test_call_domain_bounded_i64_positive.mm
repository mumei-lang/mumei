atom g(x: u32) -> i64
    requires: true;
    ensures: result == x;
    body: x

atom f(n: i64) -> i64
    requires: n >= 0 && n <= 4294967295;
    ensures: result == n;
    body: g(n)
