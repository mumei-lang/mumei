trusted atom reader(ref x: i64) -> i64
requires: true;
ensures: true;
body: x;

atom apply(g: atom_ref(i64) -> i64, v: i64) -> i64
requires: true;
ensures: true;
body: call(g, v);

atom bad() -> i64
requires: true;
ensures: true;
body: apply(atom_ref(reader), 1);
