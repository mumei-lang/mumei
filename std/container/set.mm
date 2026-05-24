// =============================================================
// std/container/set — verified set primitives
// =============================================================
// 論理セットのメンバーシップ、追加、サイズ照会を 0/1 と境界で表す。

struct BoundedSet {
    tag: i64 where v >= 0,
    cap: i64 where v > 0
}

// 論理セット内の要素存在を 0/1 witness として返す。
atom set_contains(set_tag: i64, elem: i64)
requires: set_tag >= 0 && elem >= 0;
ensures: result == 0 || result == 1;
body: {
    if set_tag == elem { 1 } else { 0 }
};

// 境界付きセットへの追加を、容量以下のサイズ witness として返す。
atom set_add(set_tag: i64, elem: i64, cap: i64)
requires: set_tag >= 0 && elem >= 0 && cap > 0;
ensures: result >= 0 && result <= cap;
body: {
    if set_tag + elem <= cap { set_tag + elem } else { cap }
};

// セット識別子から非負のサイズ witness を返す。
atom set_size(set_tag: i64)
requires: set_tag >= 0;
ensures: result >= 0;
body: {
    set_tag
};
