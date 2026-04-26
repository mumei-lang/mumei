// =============================================================
// Concurrent HTTP: task_group + Parallel Requests
// =============================================================
// task_group を使用した並行 HTTP リクエストのデモ。
// 複数の URL に同時にリクエストを送信し、結果を集約する。
//
// Plan 17: URL パラメータを Str 型に移行
// Plan 21: task / task_group は LLVM IR レベルで pthread_create /
//          pthread_join に直接 lower される（mumei-emit-llvm/src/
//          codegen.rs::compile_task_spawn を参照）。HTTP 呼び出し
//          自体は依然 stub だが、並行構造は実際に走る。
//
// Usage:
//   mumei check examples/concurrent_http.mm

import "std/http" as http;
import "std/json" as json;

// --- 単一 URL からデータを取得 ---
atom fetch_one(url: Str)
    requires: true;
    ensures: result >= 0;
    body: {
        let response = http::get(url);
        http::body_json(response)
    }

// --- 2つの URL に並行リクエスト ---
atom fetch_all(url1: Str, url2: Str)
    requires: true;
    ensures: result >= 0;
    body: {
        task_group(all) {
            task { fetch_one(url1) },
            task { fetch_one(url2) }
        }
    }

// --- 3つの URL に並行リクエストして配列に集約 ---
atom fetch_and_collect(url1: Str, url2: Str, url3: Str)
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
