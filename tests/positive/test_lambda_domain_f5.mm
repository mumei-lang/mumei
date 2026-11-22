atom g(x: u32) -> u32 requires: true; ensures: result == x; body: x
atom f(n: u32) -> u32
    requires: n == 7;
    ensures: result == 7;
    body: {
        let h = |x: u32| g(x);
        h(n)
    }
