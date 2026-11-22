type Signed = i64 where v <= 10 && forall(v, 0, 1, v >= 0);

atom f(a: i64) -> Signed
requires: a <= 10;
ensures: true;
body: { a };
