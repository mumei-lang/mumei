trusted atom g(ref x: i64)
requires: true;
ensures: true;
body: x;

trusted atom f(ref mut x: i64, y: i64)
requires: true;
ensures: true;
body: x;

atom bad(x: i64, c: bool)
requires: true;
ensures: true;
body: f(x, if c { g(x) } else { 0 });
