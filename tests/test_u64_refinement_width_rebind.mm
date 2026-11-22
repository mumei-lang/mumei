type Small = u64 where v <= 10 && forall(v, 0, 1, v < 0);

atom f(a: u64) -> Small
requires: true;
ensures: true;
body: { a };
