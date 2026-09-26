trusted atom f(ref mut a: i64, ref b: i64)
requires: true;
ensures: true;
body: a;

atom bad(x: i64)
requires: true;
ensures: true;
body: call(atom_ref(f), x, x);
