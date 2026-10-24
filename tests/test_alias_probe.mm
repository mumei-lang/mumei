// Regression: `let a = arr` aliases the array — `a[i]` must share arr's
// tracked Z3 array const and `len_arr` bound instead of starting from an
// unconstrained fresh array (which made `a[0]` unprovably in bounds).
atom array_alias_read(arr: [i64], n: i64) -> i64
requires: n >= 1 && forall(i, 0, n, arr[i] >= 0);
ensures: result >= 0;
body: {
    let a = arr;
    a[0]
};

// Store through the alias: `let a = arr` moves `arr` (arrays are Move types),
// so writes and later reads go through the alias itself.
atom array_alias_store(arr: [i64], n: i64) -> i64
requires: n >= 1 && forall(i, 0, n, arr[i] >= 0);
ensures: result == 7;
body: {
    let a = arr;
    a[0] = 7;
    a[0]
};

// Chained alias: `let b = a` where `a` already aliases `arr`.
atom array_alias_chain(arr: [i64], n: i64) -> i64
requires: n >= 1 && forall(i, 0, n, arr[i] >= 0);
ensures: result >= 0;
body: {
    let a = arr;
    let b = a;
    b[0]
};
