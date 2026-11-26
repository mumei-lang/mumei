atom f(n: u64) -> u64
requires: n == 63;
ensures: (1 << n) <= 10;
body: { n };
