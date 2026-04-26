// =============================================================
// Negative: Stmt::ArrayStore Out-of-Bounds
// =============================================================
// `arr[n] = v` のようにインデックスが len_arr を超過する可能性が
// ある場合、Z3 検証は失敗する必要がある。
atom test_array_store_oob(n: i64)
requires: n >= 0;
ensures: result == 0;
body: {
    arr[n] = 42;
    0
};
