atom invoke_spec(f: atom_ref(u32) -> i64, n: i64) -> i64
    requires: true;
    ensures: call(f, n) == n;
    body: n;
