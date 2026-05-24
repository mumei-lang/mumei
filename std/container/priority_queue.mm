// =============================================================
// std/container/priority_queue — verified priority queue
// =============================================================
// 境界付き優先度キューの len/cap と peek 操作の安全性を検証する。

struct PriorityQueue {
    len: i64 where v >= 0,
    cap: i64 where v > 0
}

// 優先度付き要素を挿入した後の要素数。
atom pq_push(len: i64, cap: i64, priority: i64)
requires: cap > 0 && len >= 0 && len < cap && priority >= 0;
ensures: result == len + 1 && result >= 1 && result <= cap;
body: {
    len + 1
};

// 空でない優先度キューから最大優先度要素を取り出した後の要素数。
atom pq_pop(len: i64)
requires: len > 0;
ensures: result == len - 1 && result >= 0;
body: {
    len - 1
};

// peek は要素数を変更せず、非空であることを維持する。
atom pq_peek(len: i64)
requires: len > 0;
ensures: result == len && result > 0;
body: {
    len
};
