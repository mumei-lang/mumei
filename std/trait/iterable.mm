// =============================================================
// std/trait/iterable — verified traversal interface
// =============================================================
// Vector/List/BoundedArray で共有する走査境界の補助 atom 群。

// Iterable の長さが非負であることを保持して返す。
atom iterable_len_nonneg(len: i64)
requires: len >= 0;
ensures: result == len && result >= 0;
body: {
    len
};

// 論理位置が範囲内なら 1、それ以外なら 0 を返す。
atom iterable_in_bounds(idx: i64, len: i64)
requires: len >= 0;
ensures: result == 0 || result == 1;
body: {
    if idx >= 0 && idx < len { 1 } else { 0 }
};

// 有効な位置を 1 つ進め、末尾境界 len までの範囲を保つ。
atom iterable_advance(idx: i64, len: i64)
requires: len >= 0 && idx >= 0 && idx < len;
ensures: result == idx + 1 && result >= 1 && result <= len;
body: {
    idx + 1
};
