// Counterexample case: off-by-one bound allows an out-of-range array index
// (i == n is accepted by the precondition).
// expected: FAIL

atom read_with_off_by_one_bound(arr: [i64], n: i64, i: i64)
requires: n >= 0 && len(arr) == n && i >= 0 && i <= n;
ensures: result == arr[i];
body: arr[i];
