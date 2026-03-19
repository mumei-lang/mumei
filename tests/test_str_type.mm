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

// --- Test 5: String concat result propagation to effect parameter ---
// Validates that a concat result stored via `let` is properly bound
// to the effect parameter (not a fresh unconstrained variable).
atom test_concat_propagation(user_id: Str)
    effects: [HttpGet(url)]
    requires: true;
    ensures: result >= 0;
    body: {
        let url = "https://api.example.com/" + user_id;
        perform HttpGet.request(url);
        1
    }
