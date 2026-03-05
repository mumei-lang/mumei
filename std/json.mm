// =============================================================
// Mumei Standard Library: std.json
// =============================================================
// JSON 文字列の解析・生成を行う FFI バックエンド標準ライブラリ。
// Rust の serde_json を隠蔽し、Mumei 側からは単純な atom 呼び出しで
// JSON 操作が可能。
//
// Usage:
//   import "std/json" as json;
//
//   let parsed = json::parse("{\"key\": 42}");
//   let value  = json::get_int(parsed, "key");
//   let output = json::stringify(parsed);
//
// 設計:
//   - JSON 値はハンドル（i64）として管理される
//   - ハンドル 0 = null / 解析失敗
//   - ハンドル > 0 = 有効な JSON オブジェクト/配列/値
//   - FFI 経由で Rust 側の serde_json::Value を操作する

// --- extern 宣言: Rust FFI バックエンド ---
extern "Rust" {
    fn json_parse(input: i64) -> i64;
    fn json_stringify(handle: i64) -> i64;
    fn json_get(handle: i64, key: i64) -> i64;
    fn json_get_int(handle: i64, key: i64) -> i64;
    fn json_get_str(handle: i64, key: i64) -> i64;
    fn json_get_bool(handle: i64, key: i64) -> i64;
    fn json_array_len(handle: i64) -> i64;
    fn json_array_get(handle: i64, index: i64) -> i64;
    fn json_is_null(handle: i64) -> i64;
    fn json_is_object(handle: i64) -> i64;
    fn json_is_array(handle: i64) -> i64;
    fn json_object_new() -> i64;
    fn json_object_set(handle: i64, key: i64, value: i64) -> i64;
    fn json_array_new() -> i64;
    fn json_array_push(handle: i64, value: i64) -> i64;
    fn json_from_int(value: i64) -> i64;
    fn json_from_str(value: i64) -> i64;
    fn json_from_bool(value: i64) -> i64;
}

// =============================================================
// Public API: JSON パース・生成
// =============================================================

// JSON 文字列をパースし、ハンドルを返す。
// 解析失敗時はハンドル 0 (null) を返す。
atom parse(input: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        json_parse(input)
    }

// JSON ハンドルを文字列に変換する。
// ハンドル 0 の場合は "null" を返す。
atom stringify(handle: i64)
    requires: handle >= 0;
    ensures: true;
    body: {
        json_stringify(handle)
    }

// =============================================================
// Public API: JSON 値の取得
// =============================================================

// オブジェクトからキーで値を取得（ハンドルを返す）
atom get(handle: i64, key: i64)
    requires: handle >= 0;
    ensures: result >= 0;
    body: {
        json_get(handle, key)
    }

// オブジェクトからキーで整数値を取得
atom get_int(handle: i64, key: i64)
    requires: handle >= 0;
    ensures: true;
    body: {
        json_get_int(handle, key)
    }

// オブジェクトからキーで文字列値を取得（ハンドルを返す）
atom get_str(handle: i64, key: i64)
    requires: handle >= 0;
    ensures: true;
    body: {
        json_get_str(handle, key)
    }

// オブジェクトからキーでブール値を取得（0=false, 1=true）
atom get_bool(handle: i64, key: i64)
    requires: handle >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        json_get_bool(handle, key)
    }

// =============================================================
// Public API: JSON 配列操作
// =============================================================

// 配列の長さを取得
atom array_len(handle: i64)
    requires: handle >= 0;
    ensures: result >= 0;
    body: {
        json_array_len(handle)
    }

// 配列からインデックスで値を取得（ハンドルを返す）
atom array_get(handle: i64, index: i64)
    requires: handle >= 0 && index >= 0;
    ensures: result >= 0;
    body: {
        json_array_get(handle, index)
    }

// =============================================================
// Public API: JSON 型判定
// =============================================================

// JSON 値が null かどうかを判定（0=false, 1=true）
atom is_null(handle: i64)
    requires: handle >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        json_is_null(handle)
    }

// JSON 値がオブジェクトかどうかを判定（0=false, 1=true）
atom is_object(handle: i64)
    requires: handle >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        json_is_object(handle)
    }

// JSON 値が配列かどうかを判定（0=false, 1=true）
atom is_array(handle: i64)
    requires: handle >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        json_is_array(handle)
    }

// =============================================================
// Public API: JSON 値の構築
// =============================================================

// 空のオブジェクトを生成
atom object_new()
    requires: true;
    ensures: result >= 0;
    body: {
        json_object_new()
    }

// オブジェクトにキーと値を設定
atom object_set(handle: i64, key: i64, value: i64)
    requires: handle >= 0;
    ensures: result >= 0;
    body: {
        json_object_set(handle, key, value)
    }

// 空の配列を生成
atom array_new()
    requires: true;
    ensures: result >= 0;
    body: {
        json_array_new()
    }

// 配列に値を追加
atom array_push(handle: i64, value: i64)
    requires: handle >= 0;
    ensures: result >= 0;
    body: {
        json_array_push(handle, value)
    }

// 整数値から JSON 値を生成
atom from_int(value: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        json_from_int(value)
    }

// 文字列から JSON 値を生成
atom from_str(value: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        json_from_str(value)
    }

// ブール値から JSON 値を生成（0=false, 1=true）
atom from_bool(value: i64)
    requires: value >= 0 && value <= 1;
    ensures: result >= 0;
    body: {
        json_from_bool(value)
    }
