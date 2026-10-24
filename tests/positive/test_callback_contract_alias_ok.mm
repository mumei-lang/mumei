atom identity(n: i64) -> i64
    requires: true;
    ensures: result == n;
    body: n;

atom apply_identity(x: i64, f: atom_ref(i64) -> i64) -> i64
    requires: true;
    ensures: result == x;
    contract(f): ensures: result == x;
    body: call(f, x);

atom use_alias_callback() -> i64
    requires: true;
    ensures: result == 0;
    body: apply_identity(0, atom_ref(identity));
