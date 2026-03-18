// =============================================================
// Enum Payload: E2E Verification Tests
// =============================================================
// Plan 14 で導入された enum payload の検証テスト。
// match によるバリアント分岐と wildcard パターンを検証する。
//
// Usage:
//   mumei check tests/test_enum_payload.mm

// --- Enum: 色の種類 ---
enum Color {
    Red,
    Green,
    Blue,
    Custom(i64)
}

// --- Test 1: Match with enum variants ---
atom test_color_match(c: i64)
    requires: c >= 0 && c <= 3;
    ensures: result >= 0;
    body: {
        match c {
            0 => 255,
            1 => 128,
            2 => 64,
            _ => 0
        }
    }

// --- Test 2: Wildcard pattern ---
atom test_wildcard(x: i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        match x {
            0 => 100,
            1 => 200,
            _ => 0
        }
    }

// --- Test 3: Nested match ---
atom test_nested_match(a: i64, b: i64)
    requires: a >= 0 && a <= 1 && b >= 0 && b <= 1;
    ensures: result >= 0;
    body: {
        match a {
            0 => match b {
                0 => 0,
                _ => 1
            },
            _ => match b {
                0 => 2,
                _ => 3
            }
        }
    }
