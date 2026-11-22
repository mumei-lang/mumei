type Small = u64 where v <= 10;
atom x(a: Small) -> u64
requires: true;
ensures: result <= 10;
body: { a };
