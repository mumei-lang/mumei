atom positive(x: i64) -> i64
    requires: x >= 0;
    ensures: result >= 0;
    body: x;

atom apply_nonneg(x: i64, f: atom_ref(i64) -> i64) -> i64
    requires: true;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, x);

atom use_bad_callback() -> i64
    requires: true;
    ensures: result >= 0;
    body: apply_nonneg(-1, atom_ref(positive));
