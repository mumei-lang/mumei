// SV-COMP style loop invariant verification
atom sum_array(arr: [i64], n: i64)
requires: n >= 0 && len(arr) >= n && forall(i, 0, n, arr[i] >= 0);
ensures: result >= 0;
body: {
    let sum = 0;
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n && sum >= 0
    decreases: n - i
    {
        sum = sum + arr[i];
        i = i + 1;
    };
    sum
};
