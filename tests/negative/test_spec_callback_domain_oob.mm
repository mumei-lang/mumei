atom use_callback(f: atom_ref(u32) -> i64) -> i64
    requires: true;
    ensures: call(f, 4294967296) >= 0;
    contract(f): ensures: result >= 0;
    body: 0;
