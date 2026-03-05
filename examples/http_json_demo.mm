// =============================================================
// Integration Demo: task_group + HTTP + JSON
// =============================================================
// このデモは Mumei の並行処理（task_group）と
// ネットワーク・ファースト標準ライブラリ（std.http, std.json）を
// 組み合わせた実用的なスクリプトの例を示す。
//
// 複数の API エンドポイントに並行リクエストを送信し、
// JSON レスポンスを解析して結果を集約する。
//
// Usage:
//   mumei check examples/http_json_demo.mm
//   mumei verify examples/http_json_demo.mm

import "std/json" as json;
import "std/http" as http;

// --- 単一 API からデータを取得して JSON 解析する atom ---
atom fetch_and_parse(url: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        let response = http::get(url);
        let data = http::body_json(response);
        data
    }

// --- レスポンスのステータスを検証する atom ---
atom check_status(url: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        let response = http::get(url);
        let code = http::status(response);
        code
    }

// --- JSON オブジェクトを構築して POST する atom ---
atom create_and_post(url: i64, name: i64, value: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        let obj = json::object_new();
        let key_handle = json::from_str(name);
        let val_handle = json::from_int(value);
        let payload = json::object_set(obj, key_handle, val_handle);
        let response = http::post(url, payload);
        http::status(response)
    }

// --- 並行リクエスト: task_group で複数 API を同時に叩く ---
// task_group(all) は全タスクの完了を待つ。
// 各タスクは独立した HTTP リクエストを実行し、
// Z3 が並行安全性（リソース競合なし）を検証する。
atom concurrent_fetch(url1: i64, url2: i64, url3: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        task_group(all) {
            task { fetch_and_parse(url1) },
            task { fetch_and_parse(url2) },
            task { fetch_and_parse(url3) }
        }
    }

// --- メインの集約 atom ---
// 並行リクエストの結果を JSON 配列に集約する。
atom aggregate_results(url1: i64, url2: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        let result1 = fetch_and_parse(url1);
        let result2 = fetch_and_parse(url2);
        let arr = json::array_new();
        let arr = json::array_push(arr, result1);
        let arr = json::array_push(arr, result2);
        let output = json::stringify(arr);
        output
    }
