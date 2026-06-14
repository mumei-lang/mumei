atom bounded_read(i: i64, n: i64) -> i64
requires: 0 <= i && i < n;
ensures: result == arr[i];
body: arr[i];
