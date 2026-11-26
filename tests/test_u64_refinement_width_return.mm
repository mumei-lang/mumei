type Small = u64 where v <= 10;
atom r(a: u64) -> Small
requires: a == 0;
ensures: true;
body: { a - 1 };
