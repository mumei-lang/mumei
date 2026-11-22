atom f(a: u64) -> u64
requires: a == 0 - 1;
ensures: ({ let ignored = 0; a }) <= 10;
body: { a };
