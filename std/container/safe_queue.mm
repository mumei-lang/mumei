// =============================================================
// Mumei Standard Library: SafeQueue
// =============================================================
// 検証済み FIFO キュー: len <= cap を不変量として保証する。
// enqueue/dequeue の安全性を精緻型と事前条件で保証する。
//
// Usage:
//   import "std/container/safe_queue" as queue;
struct SafeQueue {
    len: i64 where v >= 0,
    cap: i64 where v > 0,
    head: i64 where v >= 0,
    tail: i64 where v >= 0
}
// キューへの要素追加（オーバーフロー防止）
// requires: q_len < q_cap
// ensures: result == q_len + 1
atom enqueue(q_len: i64, q_cap: i64)
requires: q_len >= 0 && q_cap > 0 && q_len < q_cap;
ensures: result >= 0 && result <= q_cap && result == q_len + 1;
body: {
    q_len + 1
};
// キューからの要素取り出し（アンダーフロー防止）
// requires: q_len > 0
// ensures: result == q_len - 1
atom dequeue(q_len: i64)
requires: q_len > 0;
ensures: result >= 0 && result == q_len - 1;
body: {
    q_len - 1
};
// キューが空かどうか判定
atom queue_is_empty(q_len: i64)
requires: q_len >= 0;
ensures: result >= 0 && result <= 1;
body: {
    if q_len == 0 { 1 } else { 0 }
};
// キューが満杯かどうか判定
atom queue_is_full(q_len: i64, q_cap: i64)
requires: q_len >= 0 && q_cap > 0;
ensures: result >= 0 && result <= 1;
body: {
    if q_len == q_cap { 1 } else { 0 }
};
// キューの残容量を返す
atom queue_remaining(q_len: i64, q_cap: i64)
requires: q_len >= 0 && q_cap > 0 && q_len <= q_cap;
ensures: result >= 0 && result == q_cap - q_len;
body: {
    q_cap - q_len
};
// 安全な enqueue: 容量チェック付き（0=Ok, 1=Err=容量不足）
atom enqueue_safe(q_len: i64, q_cap: i64)
requires: q_len >= 0 && q_cap > 0;
ensures: result >= 0 && result <= 1;
body: {
    if q_len < q_cap { 0 } else { 1 }
};
// 安全な dequeue: 空チェック付き（0=Ok, 1=Err=空）
atom dequeue_safe(q_len: i64)
requires: q_len >= 0;
ensures: result >= 0 && result <= 1;
body: {
    if q_len > 0 { 0 } else { 1 }
};
// バッチ enqueue: count 個の要素を一括追加
atom batch_enqueue(q_len: i64, q_cap: i64, count: i64)
requires: q_len >= 0 && q_cap > 0 && count >= 0 && q_len + count <= q_cap;
ensures: result >= 0 && result == q_len + count;
body: {
    q_len + count
};
