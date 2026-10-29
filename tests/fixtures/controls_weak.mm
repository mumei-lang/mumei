atom nonnegative(x: i64, y: i64) -> i64
requires: x >= 0 && y >= 0;
ensures: result >= 0;
body: x + y + 1;
