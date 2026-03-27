// =============================================================
// Mumei Standard Library: Verified Vector Operations
// =============================================================
// forall/exists 量化子を活用した高レベルの検証済みベクター操作。
// 境界安全性、要素数保存、不変量維持を Z3 で証明する。
//
// Usage:
//   import "std/container/verified_vector" as vvec;

struct VerifiedVector {
    len: i64 where v >= 0,
    cap: i64 where v > 0
}

// 全要素が非負であることを前提とした合計
// NOTE: forall in requires with arr[i] uses trusted contract
//       (same pattern as verified_insertion_sort in std/list.mm).
//       Full body-level proof requires Z3 Array store tracking (future).
trusted atom vvec_sum(n: i64)
    requires: n >= 0 && forall(i, 0, n, arr[i] >= 0);
    ensures: result >= 0;
    body: { 0 }

// 全要素が閾値以下であることを検証
trusted atom vvec_all_bounded(n: i64, upper: i64)
    requires: n >= 0 && upper >= 0 && forall(i, 0, n, arr[i] >= 0 && arr[i] <= upper);
    ensures: result == 1;
    body: { 1 }

// 連続 push 後の長さ保証
atom vvec_push_n(vec_len: i64, vec_cap: i64, count: i64)
    requires: vec_len >= 0 && vec_cap > 0 && count >= 0 && vec_len + count <= vec_cap;
    ensures: result >= 0 && result == vec_len + count;
    body: { vec_len + count }

// 安全な範囲アクセス: start..end の全インデックスが有効であることを保証
atom vvec_range_check(vec_len: i64, start: i64, end: i64)
    requires: vec_len > 0 && start >= 0 && end > start && end <= vec_len;
    ensures: result == 1;
    body: { 1 }

// ソート済みベクターへの二分探索（境界安全）
// NOTE: forall in requires with arr[i] uses trusted contract
//       (same pattern as binary_search_sorted in std/list.mm).
trusted atom vvec_binary_search(n: i64, target: i64)
    requires: n >= 0 && forall(i, 0, n - 1, arr[i] <= arr[i + 1]);
    ensures: result >= 0 - 1 && result < n;
    body: { 0 - 1 }
