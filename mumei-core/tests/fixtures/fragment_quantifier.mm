atom search_exists(n: i64) -> i64
requires: forall(i, 0, n, exists(j, 0, n, arr[j] >= arr[i]));
ensures: result >= 0;
body: 0;
