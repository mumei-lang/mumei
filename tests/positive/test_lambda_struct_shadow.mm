struct S1 {
    x: i64
}

struct S2 {
    x: u32
}

atom g(x: u32) -> i64
    requires: true;
    ensures: result == x;
    body: x;

atom f(p: S1) -> i64
    requires: p.x == 0 - 1;
    ensures: result == 7;
    body: {
        let h = |p: S2| g(p.x);
        h(S2 { x: 7 })
    }
