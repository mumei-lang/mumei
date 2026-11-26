atom g(x: u32) -> u32 requires: true; ensures: result == x; body: x
atom f(n: i64) -> i64
    requires: n == 0 - 1;
    ensures: result == 0 - 1;
    body: {
        let h = |x: u32| g(x);
        h(n)
    }
