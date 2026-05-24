// =============================================================
// std/container/binary_heap — verified binary max-heap
// =============================================================
// 配列ベースの二分ヒープで使う index 計算と長さ更新を検証する。

struct BinaryHeap {
    len: i64 where v >= 0,
    cap: i64 where v > 0
}

// ノード i の親 index。i > 0 のため、親は常に i より小さい。
atom heap_parent(i: i64)
requires: i > 0;
ensures: result >= 0 && result < i;
body: {
    (i - 1) / 2
};

// ノード i の左子 index。呼び出し側が容量内に収まることを保証する。
atom heap_left_child(i: i64, cap: i64)
requires: cap > 0 && i >= 0 && i < 1000000 && 2 * i + 1 < cap;
ensures: result > i && result < cap;
body: {
    2 * i + 1
};

// ノード i の右子 index。呼び出し側が容量内に収まることを保証する。
atom heap_right_child(i: i64, cap: i64)
requires: cap > 0 && i >= 0 && i < 1000000 && 2 * i + 2 < cap;
ensures: result > i && result < cap;
body: {
    2 * i + 2
};

// ヒープ挿入後の要素数。
atom heap_insert_len(len: i64, cap: i64)
requires: cap > 0 && len >= 0 && len < cap;
ensures: result >= 1 && result <= cap && result == len + 1;
body: {
    len + 1
};

// 最大要素取り出し後の要素数。
atom heap_extract_len(len: i64)
requires: len > 0;
ensures: result >= 0 && result == len - 1;
body: {
    len - 1
};
