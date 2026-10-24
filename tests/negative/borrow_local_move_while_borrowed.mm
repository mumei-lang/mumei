trusted atom f(ref x: [i64], y: i64)
requires: true;
ensures: true;
body: len(x);

atom bad(x: [i64], c: bool) -> i64
requires: true;
ensures: true;
body: f(x, if c { let y = x; 0 } else { 0 });
