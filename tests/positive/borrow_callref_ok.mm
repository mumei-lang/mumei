trusted atom reader(ref x: i64) -> i64
requires: true;
ensures: true;
body: x;

trusted atom owned(x: i64) -> i64
requires: true;
ensures: true;
body: x;

atom apply(g: atom_ref(i64) -> i64, v: i64) -> i64
requires: true;
ensures: true;
body: call(g, v);

atom ok(x: i64) -> i64
requires: true;
ensures: true;
body: {
    call(atom_ref(reader), x);
    apply(atom_ref(owned), x)
};
