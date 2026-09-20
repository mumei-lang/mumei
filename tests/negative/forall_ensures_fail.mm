// =============================================================
// Negative Test: forall in ensures が不成立のケース
// =============================================================
// ensures で昇順を要求するが、body が昇順を保証しないため
// 検証が失敗することを確認する。
atom test_sorted_fail(arr: [i64], n: i64)
requires: n >= 2;
ensures: forall(i, 0, n, arr[i] <= arr[i + 1]);
body: n;
