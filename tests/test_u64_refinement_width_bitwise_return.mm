type T = u64 where (v | 0) <= 10;

atom f(a: u64) -> T
requires: a == 0;
ensures: true;
body: { a - 1 };
