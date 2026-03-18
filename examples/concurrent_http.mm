// =============================================================
// Concurrent HTTP: task_group + Parallel Requests
// =============================================================
// task_group を使用した並行 HTTP リクエストのデモ。
// 複数の URL に同時にリクエストを送信し、結果を集約する。
//
// Usage:
//   mumei check examples/concurrent_http.mm

import "std/http" as http;
import "std/json" as json;

// --- 単一 URL からデータを取得 ---
atom fetch_one(url: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        let response = http::get(url);
        http::body_json(response)
    }

// --- 2つの URL に並行リクエスト ---
atom fetch_all(url1: i64, url2: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        task_group(all) {
            task { fetch_one(url1) },
            task { fetch_one(url2) }
        }
    }

// --- 3つの URL に並行リクエストして配列に集約 ---
atom fetch_and_collect(url1: i64, url2: i64, url3: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        let r1 = fetch_one(url1);
        let r2 = fetch_one(url2);
        let r3 = fetch_one(url3);
        let arr = json::array_new();
        let arr = json::array_push(arr, r1);
        let arr = json::array_push(arr, r2);
        let arr = json::array_push(arr, r3);
        arr
    }
