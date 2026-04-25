// =============================================================
// Regression: per-array `__z3_arr_<name>` isolation
// =============================================================
// 以前は `Stmt::ArrayStore` と `Expr::ArrayAccess` が単一の `__z3_arr`
// キーを共有していたため、`brr[k] = v` の store 後に `arr[k]` を
// 読むと v が返る（= 別配列を汚染する）不健全性があった。
//
// このテストは「`brr[k] = 7` した上で `arr[k]` の値は 0 以上
// （`forall(i, 0, n, arr[i] >= 0)` の仮定のみ）」という性質を
// 検証する。もし store 追跡が配列名で分離されていなければ、
// `arr[k]` の値は 7 に固定されてしまい、要素 == 7 を返す ensures
// でも検証が通ってしまう — それを避けるため、ここでは
// `arr[k]` が 0 以上であることのみを ensures にし、
// Z3 が「`brr` への store は `arr` に影響しない」と結論できる
// ことを確認する。

atom test_multi_array_isolation(n: i64, k: i64)
requires:
    n >= 1 && k >= 0 && k < n
    && forall(i, 0, n, arr[i] == 0)
    && forall(i, 0, n, brr[i] == 0);
ensures: result == 0;
body: {
    brr[k] = 7;
    arr[k]
};

// --- 対称確認: `arr[k] = 7` の store 後でも brr は不変 ---
atom test_multi_array_isolation_rev(n: i64, k: i64)
requires:
    n >= 1 && k >= 0 && k < n
    && forall(i, 0, n, arr[i] == 0)
    && forall(i, 0, n, brr[i] == 0);
ensures: result == 0;
body: {
    arr[k] = 7;
    brr[k]
};

// --- 同一配列への store は従来どおり select で観測できる ---
atom test_same_array_store_select(n: i64, k: i64)
requires:
    n >= 1 && k >= 0 && k < n
    && forall(i, 0, n, arr[i] == 0);
ensures: result == 7;
body: {
    arr[k] = 7;
    arr[k]
};
