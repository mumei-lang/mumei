// =============================================================
// std/container/sorted_list — verified ordered sequence
// =============================================================
// ソート済みリストの挿入位置と長さ更新を検証する補助 atom 群。

struct SortedList {
    len: i64 where v >= 0,
    cap: i64 where v > 0
}

// 呼び出し側が単調性を証明済みの挿入位置を、そのまま有効範囲内の index として返す。
atom sorted_insert_position(pos: i64, len: i64)
requires: len >= 0 && pos >= 0 && pos <= len;
ensures: result == pos && result >= 0 && result <= len;
body: {
    pos
};

// ソート済みリストに 1 要素挿入した後の長さを返す。
atom sorted_insert_len(len: i64, cap: i64)
requires: cap > 0 && len >= 0 && len < cap;
ensures: result == len + 1 && result >= 1 && result <= cap;
body: {
    len + 1
};

// 隣接 2 要素が非減少順なら 1、それ以外なら 0 を返す。
atom sorted_is_ordered_pair(a: i64, b: i64)
requires: true;
ensures: result == 0 || result == 1;
body: {
    if a <= b { 1 } else { 0 }
};
