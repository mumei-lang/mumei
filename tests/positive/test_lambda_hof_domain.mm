atom g(x: u32) -> u32 requires: true; ensures: result == x; body: x

atom invoke(f: atom_ref(u32) -> u32, n: u32) -> u32
    requires: true;
    ensures: true;
    body: call(f, n);

atom caller(n: u32) -> u32
    requires: true;
    ensures: true;
    body: {
        let h = |x: u32| g(x);
        invoke(h, n)
    }
