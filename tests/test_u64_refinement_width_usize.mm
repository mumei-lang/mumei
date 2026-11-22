type Small = usize where v <= 10;
atom x_usize(a: Small) -> u64
requires: true;
ensures: result <= 10;
body: { a };
