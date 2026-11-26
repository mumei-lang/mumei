atom g(x: u32) -> i64
    requires: true;
    ensures: result >= 0 && result == x;
    body: x

atom f(n: i64) -> i64
    requires: true;
    ensures: result == 7;
    body: {
        if n < 0 { g(n) } else { 7 }
    }
