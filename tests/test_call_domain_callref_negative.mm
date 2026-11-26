atom g(x: u32) -> i64
    requires: true;
    ensures: result >= 0 && result == x;
    body: x

atom f(n: i64) -> i64
    requires: true;
    ensures: result >= 0;
    body: call(atom_ref(g), n)
