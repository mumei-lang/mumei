type Small = u64 where v <= 10;
atom callee(a: Small) -> u64
requires: true;
ensures: result == a;
body: { a };
atom v(a: u64) -> u64
requires: a == 0;
ensures: true;
body: { callee(a - 1) };
