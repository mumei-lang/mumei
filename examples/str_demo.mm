// =============================================================
// Str Demo: String Operations
// =============================================================
// Plan 9 で導入された Str 型の文字列操作デモ。
// 文字列連結・比較の基本操作を示す。
//
// Usage:
//   mumei check examples/str_demo.mm

// --- 文字列連結: 挨拶文を生成 ---
atom greet(name: Str) -> Str
    requires: true;
    ensures: true;
    body: "Hello, " + name

// --- 文字列比較: 同じ文字列かどうかを判定 ---
atom is_same(a: Str, b: Str)
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        if a == b { 1 } else { 0 }
    }

// --- 空文字列チェック ---
atom is_empty_str(s: Str)
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        if s == "" { 1 } else { 0 }
    }
