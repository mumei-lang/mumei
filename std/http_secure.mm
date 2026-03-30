// =============================================================
// Mumei Standard Library: std.http_secure
// =============================================================
// HTTPS 専用 HTTP クライアント API。
// std/http.mm と同じ FFI バックエンドを使用するが、
// パラメタライズドエフェクトにより URL が "https://" で
// 始まることをコンパイル時に強制する。
//
// Usage:
//   import "std/http_secure" as https;
//
//   let response = https::secure_get("https://api.example.com/data");
//   let status   = https::status(response);
//   let body     = https::body(response);
//
// 設計:
//   - SecureHttpGet/SecureHttpPost/SecureHttpPut/SecureHttpDelete エフェクトで
//     starts_with(url, "https://") 制約を強制
//   - std/http.mm の extern "Rust" FFI 関数を再利用
//   - ハンドルベースの API は std/http.mm と同一

// --- extern 宣言: Rust FFI バックエンド（std/http.mm と共有）---
extern "Rust" {
    fn http_get(url: Str) -> i64;
    fn http_post(url: Str, body: Str) -> i64;
    fn http_put(url: Str, body: Str) -> i64;
    fn http_delete(url: Str) -> i64;
    fn http_status(handle: i64) -> i64;
    fn http_body(handle: i64) -> Str;
    fn http_is_ok(handle: i64) -> i64;
    fn http_free(handle: i64) -> i64;
}

// --- Parameterized Effects: HTTPS URL 制約 ---
// NOTE: SecureHttpGet/SecureHttpPost は std/http.mm にも定義されている。
// mumei のエフェクトはグローバル名前空間で管理されるため、両モジュールを
// 同時にインポートした場合は後勝ち（サイレント上書き）となる。
// 制約は同一なので現在は問題ないが、将来的には std/http.mm 側の定義を
// 削除し、本モジュールに一元化することを推奨する。
effect SecureHttpGet(url: Str) where starts_with(url, "https://");
effect SecureHttpPost(url: Str) where starts_with(url, "https://");
effect SecureHttpPut(url: Str) where starts_with(url, "https://");
effect SecureHttpDelete(url: Str) where starts_with(url, "https://");

// =============================================================
// Public API: HTTPS リクエスト
// =============================================================

// HTTPS GET リクエストを送信し、レスポンスハンドルを返す。
atom secure_get(url: Str)
    effects: [SecureHttpGet(url)]
    requires: starts_with(url, "https://");
    ensures: result >= 0;
    body: {
        perform SecureHttpGet.request(url);
        http_get(url)
    }

// HTTPS POST リクエストを送信し、レスポンスハンドルを返す。
atom secure_post(url: Str, body: Str)
    effects: [SecureHttpPost(url)]
    requires: starts_with(url, "https://");
    ensures: result >= 0;
    body: {
        perform SecureHttpPost.request(url);
        http_post(url, body)
    }

// HTTPS PUT リクエストを送信し、レスポンスハンドルを返す。
atom secure_put(url: Str, body: Str)
    effects: [SecureHttpPut(url)]
    requires: starts_with(url, "https://");
    ensures: result >= 0;
    body: {
        perform SecureHttpPut.request(url);
        http_put(url, body)
    }

// HTTPS DELETE リクエストを送信し、レスポンスハンドルを返す。
atom secure_delete(url: Str)
    effects: [SecureHttpDelete(url)]
    requires: starts_with(url, "https://");
    ensures: result >= 0;
    body: {
        perform SecureHttpDelete.request(url);
        http_delete(url)
    }

// =============================================================
// Public API: レスポンス情報の取得
// =============================================================

// レスポンスの HTTP ステータスコードを取得
atom status(handle: i64)
    requires: handle >= 0;
    ensures: result >= 0;
    body: {
        http_status(handle)
    }

// レスポンスボディを Str として取得
atom body(handle: i64)
    requires: handle >= 0;
    ensures: true;
    body: {
        http_body(handle)
    }

// レスポンスが成功（2xx）かどうかを判定（0=false, 1=true）
atom is_ok(handle: i64)
    requires: handle >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        http_is_ok(handle)
    }

// =============================================================
// Memory Management
// =============================================================

// HTTP レスポンスハンドルを解放する（1=成功, 0=無効なハンドル）
atom free(handle: i64)
    requires: handle >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        http_free(handle)
    }
