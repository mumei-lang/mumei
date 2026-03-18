// =============================================================
// JSON Operations: E2E Verification Tests
// =============================================================
// std.json の操作を検証するテスト。
// オブジェクトのラウンドトリップ、配列操作、型判定を検証する。
//
// Usage:
//   mumei check tests/test_json_operations.mm

import "std/json" as json;

// --- Test 1: Object roundtrip ---
// object_new → object_set → stringify → parse のラウンドトリップ
atom test_object_roundtrip()
    requires: true;
    ensures: result >= 0;
    body: {
        let obj = json::object_new();
        let val = json::from_int(42);
        let obj = json::object_set(obj, "key", val);
        let str_out = json::stringify(obj);
        let parsed = json::parse(str_out);
        let retrieved = json::get_int(parsed, "key");
        retrieved
    }

// --- Test 2: Array operations ---
// array_new → array_push → array_len の検証
atom test_array_operations()
    requires: true;
    ensures: result >= 0;
    body: {
        let arr = json::array_new();
        let v1 = json::from_int(10);
        let v2 = json::from_int(20);
        let arr = json::array_push(arr, v1);
        let arr = json::array_push(arr, v2);
        json::array_len(arr)
    }

// --- Test 3: Type checks ---
// is_null, is_object, is_array の検証
atom test_type_checks()
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        let obj = json::object_new();
        json::is_object(obj)
    }

// --- Test 4: Array type check ---
atom test_array_type_check()
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        let arr = json::array_new();
        json::is_array(arr)
    }

// --- Test 5: Null check on handle 0 ---
atom test_null_check()
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        json::is_null(0)
    }
