// =============================================================
// Str Type: E2E Verification Tests
// =============================================================
// Plan 9 で導入された Str 型の基本操作を検証するテスト。
// 文字列連結と等価性判定を検証する。
//
// Usage:
//   mumei check tests/test_str_type.mm

// --- Test 1: String concatenation ---
atom test_str_concat() -> Str
    requires: true;
    ensures: true;
    body: "hello" + " world"

// --- Test 2: String equality ---
atom test_str_equality()
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        if "abc" == "abc" { 1 } else { 0 }
    }

// --- Test 3: String inequality ---
atom test_str_inequality()
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        if "abc" != "def" { 1 } else { 0 }
    }

// --- Test 4: Empty string comparison ---
atom test_empty_str()
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        if "" == "" { 1 } else { 0 }
    }
