atom f(a: u64) -> u64
requires: a == 0;
ensures: (result | 0) <= 10;
body: { a - 1 };
