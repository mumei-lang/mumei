atom g(x: u32) -> u32 requires: true; ensures: result == x; body: x
atom f(n: i64) -> u32
    requires: true;
    ensures: true;
    body: {
        let h = |x: u32| g(x);
        call(h, n)
    }
