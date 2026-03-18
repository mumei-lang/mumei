// =============================================================
// JSON Demo: Construction and Parsing
// =============================================================
// std.json を使用した JSON 操作のデモ。
// オブジェクト構築、配列操作、パース・シリアライズを示す。
//
// Plan 17: 文字列パラメータを Str 型に移行
//
// Usage:
//   mumei check examples/json_demo.mm

import "std/json" as json;

// --- JSON オブジェクトを構築する ---
atom build_user(name: Str, age: i64)
    requires: age >= 0;
    ensures: result >= 0;
    body: {
        let obj = json::object_new();
        let name_val = json::from_str(name);
        let age_val = json::from_int(age);
        let obj = json::object_set(obj, "name", name_val);
        let obj = json::object_set(obj, "age", age_val);
        obj
    }

// --- JSON 文字列をパースして整数値を取得 ---
atom parse_and_get_int(input: Str, key: Str)
    requires: true;
    ensures: true;
    body: {
        let parsed = json::parse(input);
        json::get_int(parsed, key)
    }

// --- JSON 配列を構築する ---
atom build_array(a: i64, b: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        let arr = json::array_new();
        let val_a = json::from_int(a);
        let val_b = json::from_int(b);
        let arr = json::array_push(arr, val_a);
        let arr = json::array_push(arr, val_b);
        arr
    }

// --- JSON オブジェクトを構築して文字列化する ---
atom build_and_stringify(key: Str, value: i64)
    requires: true;
    ensures: true;
    body: {
        let obj = json::object_new();
        let val = json::from_int(value);
        let obj = json::object_set(obj, key, val);
        json::stringify(obj)
    }
