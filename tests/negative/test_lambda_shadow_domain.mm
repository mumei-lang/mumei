atom g2(n: u32) -> i64
    requires: n <= 5;
    ensures: result == n;
    body: n;

atom apply_at_9(f: atom_ref(u32) -> i64) -> i64
    requires: true;
    ensures: result == 9;
    contract(f): ensures: result == 9;
    body: call(f, 9);

atom f(n: i64) -> i64
    requires: n == 0 - 1;
    ensures: result == 9;
    body: {
        let h = |n: u32| g2(n);
        apply_at_9(h)
    }
