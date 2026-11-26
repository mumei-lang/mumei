atom invoke(f: atom_ref(u32) -> u32, n: i64) -> u32
    requires: true;
    ensures: true;
    body: call(f, n);
