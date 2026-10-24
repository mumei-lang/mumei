struct Pair { a: i64 }

trusted atom f(ref mut a: i64, ref s: Pair) -> i64
requires: true;
ensures: true;
body: a;

atom bad(s: Pair) -> i64
requires: true;
ensures: true;
body: f(s.a, s);
