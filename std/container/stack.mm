// =============================================================
// std/container/stack — verified bounded stack
// =============================================================
// 境界付き LIFO スタックの要素数を追跡する検証済みヘルパー。
// 実データではなく len/cap の不変条件を Z3 で証明する。

struct BoundedStack {
    len: i64 where v >= 0,
    cap: i64 where v > 0
}

// push 後の要素数。len < cap により容量超過を防ぐ。
atom stack_push(len: i64, cap: i64)
requires: cap > 0 && len >= 0 && len < cap;
ensures: result >= 1 && result <= cap && result == len + 1;
body: {
    len + 1
};

// pop 後の要素数。len > 0 によりアンダーフローを防ぐ。
atom stack_pop(len: i64, cap: i64)
requires: cap > 0 && len > 0 && len <= cap;
ensures: result >= 0 && result < cap && result == len - 1;
body: {
    len - 1
};

// top 要素のインデックス。空でない場合は len - 1 が有効範囲に入る。
atom stack_peek(len: i64, cap: i64)
requires: cap > 0 && len > 0 && len <= cap;
ensures: result >= 0 && result < cap && result == len - 1;
body: {
    len - 1
};

// 空判定を 0/1 で返す。
atom stack_is_empty(len: i64)
requires: len >= 0;
ensures: result >= 0 && result <= 1;
body: {
    if len == 0 { 1 } else { 0 }
};
