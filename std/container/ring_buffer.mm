// =============================================================
// std/container/ring_buffer — verified circular buffer
// =============================================================
// 固定長リングバッファの head/tail と len/cap の境界を追跡する。

struct RingBuffer {
    len: i64 where v >= 0,
    cap: i64 where v > 0,
    head: i64 where v >= 0,
    tail: i64 where v >= 0
}

// インデックスを 1 つ進め、末尾なら 0 に戻す。
atom ring_advance(idx: i64, cap: i64)
requires: cap > 0 && idx >= 0 && idx < cap;
ensures: result >= 0 && result < cap;
body: {
    if idx + 1 < cap { idx + 1 } else { 0 }
};

// 余裕のあるリングバッファへ push した後の長さ。
atom ring_push(len: i64, cap: i64)
requires: cap > 0 && len >= 0 && len < cap;
ensures: result >= 1 && result <= cap && result == len + 1;
body: {
    len + 1
};

// 空でないリングバッファから pop した後の長さ。
atom ring_pop(len: i64, cap: i64)
requires: cap > 0 && len > 0 && len <= cap;
ensures: result >= 0 && result < cap && result == len - 1;
body: {
    len - 1
};

// 空判定を 0/1 で返す。
atom ring_is_empty(len: i64)
requires: len >= 0;
ensures: result >= 0 && result <= 1;
body: {
    if len == 0 { 1 } else { 0 }
};

// 満杯判定を 0/1 で返す。
atom ring_is_full(len: i64, cap: i64)
requires: cap > 0 && len >= 0 && len <= cap;
ensures: result >= 0 && result <= 1;
body: {
    if len == cap { 1 } else { 0 }
};
