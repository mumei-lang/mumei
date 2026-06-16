atom linear_scale(x: i64) -> i64
requires: x >= 0;
ensures: result == 3 * x && result >= x;
body: 3 * x;
