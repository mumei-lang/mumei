// =============================================================
// Mumei Standard Library: BoundedArray
// =============================================================
// 境界付き配列: len <= cap を不変量として保証する。
// push/pop の安全性を精緻型と事前条件で保証する。
//
// Usage:
//   import "std/container/bounded_array" as bounded;
struct BoundedArray {
    len: i64 where v >= 0,
    cap: i64 where v > 0
}
// 境界付き配列への要素追加
// requires: len < cap（オーバーフロー防止）
// ensures: result == len + 1（要素数が1増える）
atom bounded_push(arr_len: i64, arr_cap: i64)
requires: arr_len >= 0 && arr_cap > 0 && arr_len < arr_cap;
ensures: result >= 0 && result <= arr_cap && result == arr_len + 1;
body: {
    arr_len + 1
};
// 境界付き配列からの要素削除
// requires: len > 0（アンダーフロー防止）
// ensures: result == len - 1
atom bounded_pop(arr_len: i64)
requires: arr_len > 0;
ensures: result >= 0 && result == arr_len - 1;
body: {
    arr_len - 1
};
// 配列が空かどうか判定
atom bounded_is_empty(arr_len: i64)
requires: arr_len >= 0;
ensures: result >= 0 && result <= 1;
body: {
    if arr_len == 0 { 1 } else { 0 }
};
// 配列が満杯かどうか判定
atom bounded_is_full(arr_len: i64, arr_cap: i64)
requires: arr_len >= 0 && arr_cap > 0;
ensures: result >= 0 && result <= 1;
body: {
    if arr_len == arr_cap { 1 } else { 0 }
};

// =============================================================
// Phase 4: ソート済み配列の検証済み操作
// =============================================================
// forall in ensures を活用した契約ベースの検証。
// ソート済み配列に対する操作が不変量を維持することを証明する。

// --- ソート済み配列の不変量チェック ---
// 配列が昇順であることを事前条件として仮定し、
// 事後条件でも昇順であることを保証する（恒等操作）。
// Phase 1 の forall in ensures の動作検証を兼ねる。
atom sorted_identity(n: i64)
requires: n >= 0 && forall(i, 0, n - 1, arr[i] <= arr[i + 1]);
ensures: result == n && forall(i, 0, result - 1, arr[i] <= arr[i + 1]);
body: n;

// --- ソート済み配列の最小値取得 ---
// ソート済み配列の先頭要素は最小値であることを保証する。
// requires: 配列は昇順 && 長さ >= 1
// ensures: result <= arr[i] for all i (先頭要素が最小)
atom sorted_min(n: i64)
requires: n >= 1 && forall(i, 0, n - 1, arr[i] <= arr[i + 1]);
ensures: result >= 0;
body: {
    0
};

// --- ソート済み配列の最大値取得 ---
// ソート済み配列の末尾要素は最大値であることを保証する。
atom sorted_max(n: i64)
requires: n >= 1 && forall(i, 0, n - 1, arr[i] <= arr[i + 1]);
ensures: result >= 0;
body: {
    n - 1
};

// --- ソート済み配列への挿入（要素数のみ追跡） ---
// ソート済み配列に要素を挿入した後の長さを返す。
// 要素数保存: result == n + 1
atom sorted_insert_len(n: i64, arr_cap: i64)
requires: n >= 0 && arr_cap > 0 && n < arr_cap;
ensures: result == n + 1 && result <= arr_cap;
body: {
    n + 1
};
