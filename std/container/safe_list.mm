// =============================================================
// Mumei Standard Library: SafeList
// =============================================================
// 検証済み可変長リスト: len <= cap を不変量として保証する。
// 境界チェック付きの get/set/push/pop 操作を提供する。
// `std/container/safe_queue.mm` および `std/container/bounded_array.mm`
// と同じ契約スタイル (要素数のみを追跡し、事前条件で境界を担保する)。
//
// Usage:
//   import "std/container/safe_list" as list;
struct SafeList {
    len: i64 where v >= 0,
    cap: i64 where v > 0
}
// インデックス指定で要素を取得（境界チェック付き）
// requires: 0 <= index < list_len
// ensures: result >= 0 && result < list_len
//   （safe_queue.mm の慣例に合わせ、戻り値はインデックス値そのもの）
atom safe_get(list_len: i64, index: i64)
requires: list_len > 0 && index >= 0 && index < list_len;
ensures: result >= 0 && result < list_len;
body: {
    index
};
// インデックス指定で要素を設定（境界チェック付き）
// requires: 0 <= index < list_len
// ensures: result == list_len（長さは変化しない）
atom safe_set(list_len: i64, index: i64)
requires: list_len > 0 && index >= 0 && index < list_len;
ensures: result == list_len;
body: {
    list_len
};
// 末尾に要素を追加（オーバーフロー防止）
// requires: list_len < list_cap
// ensures: result == list_len + 1
atom safe_push(list_len: i64, list_cap: i64)
requires: list_len >= 0 && list_cap > 0 && list_len < list_cap;
ensures: result >= 0 && result <= list_cap && result == list_len + 1;
body: {
    list_len + 1
};
// 末尾の要素を削除（アンダーフロー防止）
// requires: list_len > 0
// ensures: result == list_len - 1
atom safe_pop(list_len: i64)
requires: list_len > 0;
ensures: result >= 0 && result == list_len - 1;
body: {
    list_len - 1
};
// リストが空かどうか判定（0=否, 1=空）
atom safe_list_is_empty(list_len: i64)
requires: list_len >= 0;
ensures: result >= 0 && result <= 1;
body: {
    if list_len == 0 { 1 } else { 0 }
};
// リストが満杯かどうか判定（0=否, 1=満杯）
atom safe_list_is_full(list_len: i64, list_cap: i64)
requires: list_len >= 0 && list_cap > 0;
ensures: result >= 0 && result <= 1;
body: {
    if list_len == list_cap { 1 } else { 0 }
};
// 残容量を返す
// requires: list_len <= list_cap
// ensures: result == list_cap - list_len
atom safe_list_remaining(list_len: i64, list_cap: i64)
requires: list_len >= 0 && list_cap > 0 && list_len <= list_cap;
ensures: result >= 0 && result == list_cap - list_len;
body: {
    list_cap - list_len
};
// 安全な get: インデックスが範囲内なら 0（Ok）、範囲外なら 1（Err）を返す（panic しない）
atom safe_get_checked(list_len: i64, index: i64)
requires: list_len >= 0;
ensures: result >= 0 && result <= 1;
body: {
    if index >= 0 && index < list_len { 0 } else { 1 }
};
