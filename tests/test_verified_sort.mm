// =============================================================
// Test: 実 body を持つ挿入ソート（trusted）
// =============================================================
// std/list.mm の verified_insertion_sort / verified_merge_sort を
// 実 `arr[i] = val` body に置き換えた後の回帰テスト。
//
// 備考: Z3 の Array theory + forall の組み合わせは solver timeout を
// 起こしやすい。insertion sort / merge sort の完全な関数的正当性
// 証明は将来課題で、本テストでは `trusted` を付けた上で構文・
// Array::store 追跡・lowering が通ることを確認する。

// --- Test 1: 要素数保存（身元関数） ---
// 実 body で `arr[i] = arr[i]` を走らせても、result == n が成立することを
// 確認する。Z3 は店舗後の配列要素を要素そのものとして解釈できる。
// trusted を外せるように len_arr >= n の forall 前提を追加。
// MIR move 解析と path 条件伝播の修正により、要素数保存契約が
// `trusted` 不要で証明できるようになった。
atom verify_noop_sort(n: i64)
requires: n >= 0 && forall(i, 0, n, arr[i] >= 0);
ensures: result == n;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        arr[i] = arr[i];
        i = i + 1;
    };
    n
};

// --- Test 2: 挿入ソートの構造 ---
// `arr[j-1]` の OOB を回避するため `forall(i, 0, n, arr[i] >= 0)` を
// requires に付けて len_arr >= n を確保する。要素数保存契約は
// MIR move 解析改善 + path 条件伝播 + forall パターン強化により
// `trusted` 不要で証明できる。
atom verify_insertion_sort_skeleton(n: i64)
requires: n >= 0 && forall(i, 0, n, arr[i] >= 0);
ensures: result == n;
body: {
    if n <= 1 { n }
    else {
        let i = 1;
        while i < n
        invariant: i >= 1 && i <= n
        decreases: n - i
        {
            let key = arr[i];
            let j = i;
            while j > 0
            invariant: j >= 0 && j <= i
            decreases: j
            {
                if arr[j - 1] > key {
                    arr[j] = arr[j - 1];
                    j = j - 1
                } else {
                    j = 0
                }
            };
            arr[j] = key;
            i = i + 1
        };
        n
    }
};
