atom positive(x: i64) -> i64
    requires: x >= 0;
    ensures: result >= 0;
    body: x;

atom apply(f: atom_ref(i64) -> i64) -> i64
    requires: true;
    ensures: true;
    contract(f): requires: x >= 0;
    body: call(f, -1);

atom use() -> i64
    requires: true;
    ensures: true;
    body: apply(atom_ref(positive));
