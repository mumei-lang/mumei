// =============================================================
// std/container/deque — verified double-ended queue
// =============================================================
// 両端キューの len/cap 境界を追跡する bookkeeping atom 群。

struct Deque {
    len: i64 where v >= 0,
    cap: i64 where v > 0
}

atom deque_push_front(len: i64, cap: i64)
requires: cap > 0 && len >= 0 && len < cap;
ensures: result >= 1 && result <= cap && result == len + 1;
body: {
    len + 1
};

atom deque_push_back(len: i64, cap: i64)
requires: cap > 0 && len >= 0 && len < cap;
ensures: result >= 1 && result <= cap && result == len + 1;
body: {
    len + 1
};

atom deque_pop_front(len: i64)
requires: len > 0;
ensures: result >= 0 && result == len - 1;
body: {
    len - 1
};

atom deque_pop_back(len: i64)
requires: len > 0;
ensures: result >= 0 && result == len - 1;
body: {
    len - 1
};

atom deque_is_empty(len: i64)
requires: len >= 0;
ensures: result >= 0 && result <= 1;
body: {
    if len == 0 { 1 } else { 0 }
};

atom deque_is_full(len: i64, cap: i64)
requires: cap > 0 && len >= 0 && len <= cap;
ensures: result >= 0 && result <= 1;
body: {
    if len == cap { 1 } else { 0 }
};

atom deque_remaining(len: i64, cap: i64)
requires: cap > 0 && len >= 0 && len <= cap;
ensures: result >= 0 && result == cap - len;
body: {
    cap - len
};
