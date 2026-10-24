atom zero(x: i64) -> i64
    requires: true;
    ensures: result == 0;
    body: 0;

atom apply_nonneg(x: i64, f: atom_ref(i64) -> i64) -> i64
    requires: true;
    ensures: result >= 0;
    contract(f): ensures: result >= 0;
    body: call(f, x);

atom use_good_callback() -> i64
    requires: true;
    ensures: result >= 0;
    body: apply_nonneg(0, atom_ref(zero));
