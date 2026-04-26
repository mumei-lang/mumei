// =============================================================
// Test: ensures 内 forall + arr[i] パターン強化
// =============================================================
// Z3 forall の E-matching パターンが requires 側だけでなく
// ensures 側にも適用されるようになったことの回帰試験。
//
// 検証する性質:
//   1. ensures 内 `forall(i, 0, n, arr[i] >= 0)` が
//      requires と同じ `arr[i]` パターンで Z3 にインスタンス化される
//   2. body 内の `arr[idx] = v` 後でも post-store 配列状態に対して
//      forall が成立することを Z3 が証明できる

// --- Test 1: identity-on-arrays ---
// 入力配列を変更しなければ ensures の forall は requires と同型なので
// pattern 強化なしでも通るが、smt.mbqi / qi.eager_threshold の
// 自動チューニングが効いていないと timeout しやすいケース。
atom forall_ensures_identity(n: i64)
requires: n >= 0 && forall(i, 0, n, arr[i] >= 0);
ensures: result == n && forall(i, 0, n, arr[i] >= 0);
body: n;

// --- Test 2: store では森を変えない ---
// `arr[k]` を一度だけ書くが、書く値もまた >= 0 なので、
// ensures の `forall(i, 0, n, arr[i] >= 0)` は post-store 配列に対して
// E-matching pattern (`select(__z3_arr_arr, i)`) を介してインスタンス化されないと
// 証明できない。
atom forall_ensures_after_store(n: i64, k: i64)
requires: n >= 0 && k >= 0 && k < n && forall(i, 0, n, arr[i] >= 0);
ensures: result == n && forall(i, 0, n, arr[i] >= 0);
body: {
    arr[k] = 42;
    n
};

// --- Test 3: ループ store でも forall が保たれる ---
// 0..n を 0 で埋め直す。ensures の forall は post-store 配列を参照する。
atom forall_ensures_loop_zero(n: i64)
requires: n >= 0 && forall(i, 0, n, arr[i] >= 0);
ensures: result == n && forall(i, 0, n, arr[i] >= 0);
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        arr[i] = 0;
        i = i + 1;
    };
    n
};
