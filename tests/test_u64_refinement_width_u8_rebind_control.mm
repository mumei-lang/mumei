type Small = u8 where v <= 10 && forall(v, 0, 1, v >= 0);

atom f(a: u8) -> Small
requires: a <= 10;
ensures: true;
body: { a };
