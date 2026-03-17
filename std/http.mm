// =============================================================
// Mumei Standard Library: std.http
// =============================================================
// HTTP クライアント機能を提供する FFI バックエンド標準ライブラリ。
// Rust の reqwest を隠蔽し、Mumei 側からは単純な atom 呼び出しで
// HTTP リクエストが可能。
//
// Usage:
//   import "std/http" as http;
//
//   let response = http::get("https://api.example.com/data");
//   let status   = http::status(response);
//   let body     = http::body(response);
//
// 設計:
//   - Response はハンドル（i64）として管理される
//   - ハンドル 0 = リクエスト失敗（ネットワークエラー等）
//   - ハンドル > 0 = 有効なレスポンスオブジェクト
//   - task_group と組み合わせて並行リクエストが可能
//   - FFI 経由で Rust 側の reqwest を操作する
//   - Plan 11: Str 型対応 — URL/ボディ/ヘッダーは Str として直接渡す

// --- extern 宣言: Rust FFI バックエンド ---
// Plan 11: Str 型に移行 — URL/ボディ/ヘッダーパラメータを i64 から Str に変更
extern "Rust" {
    fn http_get(url: Str) -> i64;
    fn http_post(url: Str, body: Str) -> i64;
    fn http_put(url: Str, body: Str) -> i64;
    fn http_delete(url: Str) -> i64;
    fn http_status(handle: i64) -> i64;
    fn http_body(handle: i64) -> Str;
    fn http_body_json(handle: i64) -> i64;
    fn http_header_get(handle: i64, name: Str) -> Str;
    fn http_header_set(handle: i64, name: Str, value: Str) -> i64;
    fn http_is_ok(handle: i64) -> i64;
    fn http_is_error(handle: i64) -> i64;
}

// =============================================================
// Public API: HTTP リクエスト
// =============================================================

// GET リクエストを送信し、レスポンスハンドルを返す。
// Plan 11: url を Str 型に変更
// ネットワークエラー時はハンドル 0 を返す。
atom get(url: Str)
    effects: [HttpGet(url)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform HttpGet.request(url);
        http_get(url)
    }

// POST リクエストを送信し、レスポンスハンドルを返す。
// Plan 11: url, body を Str 型に変更
atom post(url: Str, body: Str)
    effects: [HttpPost(url)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform HttpPost.request(url);
        http_post(url, body)
    }

// PUT リクエストを送信し、レスポンスハンドルを返す。
// Plan 11: url, body を Str 型に変更
atom put(url: Str, body: Str)
    effects: [HttpPut(url)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform HttpPut.request(url);
        http_put(url, body)
    }

// DELETE リクエストを送信し、レスポンスハンドルを返す。
// Plan 11: url を Str 型に変更
atom delete(url: Str)
    effects: [HttpDelete(url)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform HttpDelete.request(url);
        http_delete(url)
    }

// =============================================================
// Public API: レスポンス情報の取得
// =============================================================

// レスポンスの HTTP ステータスコードを取得（200, 404, 500 等）
atom status(handle: i64)
    requires: handle >= 0;
    ensures: result >= 0;
    body: {
        http_status(handle)
    }

// レスポンスボディを Str として取得
// Plan 11: 戻り値を Str に変更
atom body(handle: i64)
    requires: handle >= 0;
    ensures: true;
    body: {
        http_body(handle)
    }

// レスポンスボディを JSON ハンドルとしてパースして取得
// Content-Type が application/json の場合に使用
atom body_json(handle: i64)
    requires: handle >= 0;
    ensures: result >= 0;
    body: {
        http_body_json(handle)
    }

// =============================================================
// Public API: ヘッダー操作
// =============================================================

// レスポンスヘッダーの値を取得
// Plan 11: name を Str 型に変更、戻り値も Str
atom header_get(handle: i64, name: Str)
    requires: handle >= 0;
    ensures: true;
    body: {
        http_header_get(handle, name)
    }

// リクエストヘッダーを設定（新しいハンドルを返す）
// Plan 11: name, value を Str 型に変更
atom header_set(handle: i64, name: Str, value: Str)
    requires: handle >= 0;
    ensures: result >= 0;
    body: {
        http_header_set(handle, name, value)
    }

// =============================================================
// Public API: レスポンス判定
// =============================================================

// レスポンスが成功（2xx）かどうかを判定（0=false, 1=true）
atom is_ok(handle: i64)
    requires: handle >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        http_is_ok(handle)
    }

// レスポンスがエラーかどうかを判定（0=false, 1=true）
// ハンドル 0（ネットワークエラー）も true を返す
atom is_error(handle: i64)
    requires: handle >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        http_is_error(handle)
    }
