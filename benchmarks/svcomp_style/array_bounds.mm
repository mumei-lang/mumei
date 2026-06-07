// SV-COMP style array bounds verification
atom safe_array_access(arr: [i64], n: i64, i: i64)
requires: n >= 0 && len(arr) >= n && i >= 0 && i < n;
ensures: result == arr[i];
body: arr[i];
