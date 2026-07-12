atom safe_add_with_bound(x: i64, y: i64) -> i64
requires: x >= 0 && y >= 0 && x + y <= 2**64 - 1;
ensures: 0 <= result <= 2**64 - 1 && result == x + y;
body: x + y;
