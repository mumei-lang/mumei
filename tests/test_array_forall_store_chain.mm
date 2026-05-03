// =============================================================
// PR 1: forall + Array store chain regression
// =============================================================
// Several consecutive `arr[k] = v` stores should not break the
// `forall(i, 0, n, arr[i] >= 0)` ensures, because each value being
// written is itself non-negative.
//
// This pins the behaviour of `configure_array_quantifier_params` —
// the centralised `mbqi` opt-in that powers `forall + store` reasoning.

// --- 1: two consecutive non-negative stores ---
atom forall_store_chain_two(n: i64, k1: i64, k2: i64)
requires:
    n >= 0
    && k1 >= 0 && k1 < n
    && k2 >= 0 && k2 < n
    && forall(i, 0, n, arr[i] >= 0);
ensures: result == n && forall(i, 0, n, arr[i] >= 0);
body: {
    arr[k1] = 1;
    arr[k2] = 2;
    n
};

// --- 2: three consecutive non-negative stores ---
atom forall_store_chain_three(n: i64, k1: i64, k2: i64, k3: i64)
requires:
    n >= 0
    && k1 >= 0 && k1 < n
    && k2 >= 0 && k2 < n
    && k3 >= 0 && k3 < n
    && forall(i, 0, n, arr[i] >= 0);
ensures: result == n && forall(i, 0, n, arr[i] >= 0);
body: {
    arr[k1] = 7;
    arr[k2] = 8;
    arr[k3] = 9;
    n
};

// --- 3: store inside if-branch ---
// The stored value is 0 in both arms, so the forall is preserved.
atom forall_store_chain_branch(n: i64, k: i64, flag: i64)
requires:
    n >= 0
    && k >= 0 && k < n
    && forall(i, 0, n, arr[i] >= 0);
ensures: result == n && forall(i, 0, n, arr[i] >= 0);
body: {
    if flag >= 0 {
        arr[k] = 0
    } else {
        arr[k] = 0
    };
    n
};
