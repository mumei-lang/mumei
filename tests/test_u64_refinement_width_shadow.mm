atom f(a: u64, c: bool) -> i64
requires: true;
ensures: (if c { let a = 0 - 1; a } else { 0 - 1 }) < 0;
body: { 0 };
