// =============================================================
// HTTP Demo: GET + Response Processing
// =============================================================
// std.http を使用した HTTP リクエストのデモ。
// GET リクエストを送信し、レスポンスのステータス・ボディ・JSON を取得する。
//
// Usage:
//   mumei check examples/http_demo.mm

import "std/http" as http;

// --- GET リクエストのステータスコードを取得 ---
atom fetch_status(url: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        let response = http::get(url);
        http::status(response)
    }

// --- GET リクエストのボディを取得 ---
atom fetch_body(url: i64)
    requires: true;
    ensures: true;
    body: {
        let response = http::get(url);
        http::body(response)
    }

// --- GET リクエストのボディを JSON としてパース ---
atom fetch_json(url: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        let response = http::get(url);
        http::body_json(response)
    }

// --- レスポンスの成功/失敗を判定 ---
atom check_ok(url: i64)
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        let response = http::get(url);
        http::is_ok(response)
    }
