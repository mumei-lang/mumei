trusted atom g(ref a: i64, b: i64)
requires: true;
ensures: true;
body: a;

atom bad(x: i64)
requires: true;
ensures: true;
body: g(x, { x = 5; x });
