trusted atom f(ref x: [i64], y: [i64])
consume y;
requires: true;
ensures: true;
body: len(x);

atom bad(x: [i64])
requires: true;
ensures: true;
body: f(x, x);
