atom g(xs: [u32]) -> i64
    requires: true;
    ensures: result >= 0;
    body: 0

atom f(arr: [u32]) -> i64
    requires: true;
    ensures: result >= 0;
    body: g(arr)
