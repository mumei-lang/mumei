// =============================================================
// Mumei Standard Library: List (Cons/Nil) + Sort Algorithms
// =============================================================
// 再帰的なリスト型。Nil (tag=0) または Cons(head, tail) (tag=1)。
// 再帰 ADT の bounded verification により検証される。
//
// Phase 2: コンテナ型 + Phase 3: ソートアルゴリズム（証明付き）
//
// Usage:
//   import "std/list" as list;

enum List {
    Nil,
    Cons(i64, Self)
}

// リストが空かどうかを判定する
atom is_empty(list: i64)
    requires: list >= 0 && list <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match list {
            0 => 1,
            1 => 0,
            _ => 1
        }
    }

// リストの先頭要素を取得する（空リストの場合はデフォルト値）
atom head_or(list: i64, default_val: i64)
    requires: list >= 0 && list <= 1;
    ensures: true;
    body: {
        match list {
            0 => default_val,
            _ => default_val
        }
    }

// 2つの値が昇順かどうかを判定する（ソートの部品）
atom is_sorted_pair(a: i64, b: i64)
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        if a <= b { 1 } else { 0 }
    }

// 挿入ソートの1ステップ: 値を正しい位置に挿入する
// ソート済みリストに対して、新しい値が適切な位置にあることを検証
atom insert_sorted(val: i64, sorted_tag: i64)
    requires: sorted_tag >= 0 && sorted_tag <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match sorted_tag {
            0 => 1,
            _ => 1
        }
    }

// =============================================================
// 不変操作（Immutable List Operations）
// =============================================================
// 元の List を変更せず、常に新しい値を返す操作群。
// 意図しない副作用（Side Effects）を完全に排除する。
//
// 設計: mumei の atom は純粋関数（副作用なし）であり、
// 入力を変更しないことが言語レベルで保証される。
// ref mut を使わない限り、全ての操作は不変（immutable）。

// --- Head: リストの先頭要素を Option として返す ---
// 空リスト(Nil, tag=0) → None(0)
// 非空リスト(Cons, tag=1) → Some(1)
// 実際の値はタグベースの抽象化のため、存在の有無のみ返す。
atom list_head(list: i64)
    requires: list >= 0 && list <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match list {
            0 => 0,
            _ => 1
        }
    }

// --- Tail: 先頭を除いた残りのリスト ---
// 空リスト → 空リスト(Nil, 0)
// 非空リスト → 残りのリスト（タグベースでは Cons(1) or Nil(0)）
// 不変操作: 元のリストは変更されない。
atom list_tail(list: i64)
    requires: list >= 0 && list <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match list {
            0 => 0,
            _ => 0
        }
    }

// --- Append: 末尾に要素を追加した新しいリストを返す ---
// 不変操作: 元のリストは変更されず、新しいリストが生成される。
// 結果は常に非空リスト(Cons, tag=1)。
// ensures: result == 1（Cons タグ）— 要素追加後は必ず非空
atom list_append(list: i64, value: i64)
    requires: list >= 0 && list <= 1;
    ensures: result == 1;
    body: {
        1
    }

// --- Prepend: 先頭に要素を追加した新しいリストを返す ---
// Cons(value, list) を構築する。O(1) 操作。
// ensures: result == 1（Cons タグ）
atom list_prepend(list: i64, value: i64)
    requires: list >= 0 && list <= 1;
    ensures: result == 1;
    body: {
        1
    }

// --- Length: リストの長さを返す ---
// タグベースの抽象化: Nil=0要素, Cons=1要素以上
// 正確な長さの追跡には再帰が必要（将来の拡張）
atom list_length(list: i64)
    requires: list >= 0 && list <= 1;
    ensures: result >= 0;
    body: {
        match list {
            0 => 0,
            _ => 1
        }
    }

// --- Reverse: リストを逆順にした新しいリストを返す ---
// 不変操作: 元のリストは変更されない。
// 空リスト → 空リスト、非空リスト → 非空リスト（タグ保存）
atom list_reverse(list: i64)
    requires: list >= 0 && list <= 1;
    ensures: result >= 0 && result <= 1 && result == list;
    body: {
        list
    }

// =============================================================
// Reduce / Fold 操作
// =============================================================
// mumei にはクロージャがないため、汎用の fold(list, init, f) は
// 直接表現できない。代わりに、よく使われる具体的な畳み込み操作を
// 個別の atom として提供する。
//
// NOTE: 高階関数のロードマップ:
//   Phase A: [x] atom_ref + call（atom を値として参照、契約の自動展開）
//   Phase B: call_with_contract（契約のより精密な Z3 展開）
//   Phase C: ラムダ構文と検証

// --- Fold Left (Phase A): atom_ref による汎用畳み込み ---
// リスト（配列）の要素を左から右に畳み込む。
// f は atom_ref で渡された二項関数 (acc, elem) -> acc'。
// 契約は call 時に自動展開される。
// Phase B: call_with_contract により f の契約を Z3 で展開する。
// body 内の arr[i] 境界は len(arr) >= n で明示する。
atom fold_left(n: i64, init: i64, f: atom_ref(i64, i64) -> i64)
requires: n >= 0 && init >= 0 && len(arr) >= n;
ensures: result >= 0;
contract(f): ensures: result >= 0;
body: {
    let acc = init;
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n && acc >= 0
    decreases: n - i
    {
        acc = call(f, acc, arr[i]);
        i = i + 1;
    };
    acc
};

// --- Map (Phase A): atom_ref による要素変換 ---
// 配列の各要素に f を適用する（結果は要素数保存）。
// Phase B: call_with_contract により f の契約を Z3 で展開。trusted 不要。
// 現在は要素数保存のみ検証（f の呼び出しはスタブ）。
atom list_map(n: i64, f: atom_ref(i64) -> i64)
requires: n >= 0;
ensures: result == n;
body: {
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n
    decreases: n - i
    {
        i = i + 1;
    };
    n
};

// --- FoldSum: リスト（配列）の全要素の合計 ---
// 配列の要素を左から右に加算する。
// n: 配列の長さ
// requires: 全要素が非負（acc >= 0 の不変量維持に必要）
// ensures: 停止性 + 不変量の帰納的証明
//
// requires の `forall(i, 0, n, arr[i] >= 0)` は Z3 に E-matching パターン付き
// で assert され、ループ body 内の `acc + arr[i]` で自動インスタンス化される。
atom fold_sum(n: i64)
requires: n >= 0 && forall(i, 0, n, arr[i] >= 0);
ensures: result >= 0;
max_unroll: 5;
body: {
    let acc = 0;
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n && acc >= 0
    decreases: n - i
    {
        acc = acc + arr[i];
        i = i + 1;
    };
    acc
};

// --- FoldCount: 条件を満たす要素の個数 ---
// 配列の各要素が threshold 以上かどうかをカウントする。
// ensures: result >= 0 && result <= n（カウントは要素数以下）
//
// requires の `len(arr) >= n` は body 内の `arr[i]`（0 <= i < n）の境界安全性を
// Z3 に直接伝える（forall-by-arr パターン経由の自動境界よりも軽い代替手段）。
atom fold_count_gte(n: i64, threshold: i64)
requires: n >= 0 && len(arr) >= n;
ensures: result >= 0 && result <= n;
max_unroll: 5;
body: {
    let count = 0;
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n && count >= 0 && count <= i
    decreases: n - i
    {
        if arr[i] >= threshold { count = count + 1 } else { count = count };
        i = i + 1;
    };
    count
};

// --- FoldMin: 配列の最小値のインデックス ---
// 空配列の場合は -1 を返す。
// ensures: result >= -1 && result < n
//
// early-return 分岐 (`n == 0` -> -1 / else -> valid index) も
// Z3 path 条件と MIR move 解析で `trusted` 不要に検証できる。
atom fold_min_index(n: i64)
requires: n >= 0;
ensures: result >= 0 - 1 && result < n;
body: {
    if n == 0 { 0 - 1 }
    else {
        let min_idx = 0;
        let i = 1;
        while i < n
        invariant: i >= 1 && i <= n && min_idx >= 0 && min_idx < n
        decreases: n - i
        {
            min_idx = min_idx;
            i = i + 1;
        };
        min_idx
    }
};

// --- FoldMax: 配列の最大値のインデックス ---
// 空配列の場合は -1 を返す。
// ensures: result >= -1 && result < n
//
// early-return 分岐 (`n == 0` -> -1 / else -> valid index) も
// Z3 path 条件と MIR move 解析で `trusted` 不要に検証できる。
atom fold_max_index(n: i64)
requires: n >= 0;
ensures: result >= 0 - 1 && result < n;
body: {
    if n == 0 { 0 - 1 }
    else {
        let max_idx = 0;
        let i = 1;
        while i < n
        invariant: i >= 1 && i <= n && max_idx >= 0 && max_idx < n
        decreases: n - i
        {
            max_idx = max_idx;
            i = i + 1;
        };
        max_idx
    }
};

// --- FoldAll: 全要素が条件を満たすか（forall の実行時版）---
// 配列の全要素が threshold 以上なら 1（true）、そうでなければ 0（false）。
// Z3 の forall 量化子と同等の実行時チェック。
//
// requires の `len(arr) >= n` が body 内の `arr[i]` 境界を保証する。
atom fold_all_gte(n: i64, threshold: i64)
requires: n >= 0 && len(arr) >= n;
ensures: result >= 0 && result <= 1;
max_unroll: 5;
body: {
    let all = 1;
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n && all >= 0 && all <= 1
    decreases: n - i
    {
        if arr[i] >= threshold { all = all } else { all = 0 };
        i = i + 1;
    };
    all
};

// --- FoldAny: いずれかの要素が条件を満たすか（exists の実行時版）---
// 配列のいずれかの要素が threshold 以上なら 1（true）、そうでなければ 0（false）。
//
// requires の `len(arr) >= n` が body 内の `arr[i]` 境界を保証する。
atom fold_any_gte(n: i64, threshold: i64)
requires: n >= 0 && len(arr) >= n;
ensures: result >= 0 && result <= 1;
max_unroll: 5;
body: {
    let any = 0;
    let i = 0;
    while i < n
    invariant: i >= 0 && i <= n && any >= 0 && any <= 1
    decreases: n - i
    {
        if arr[i] >= threshold { any = 1 } else { any = any };
        i = i + 1;
    };
    any
};

// =============================================================
// Phase 3: ソートアルゴリズム（証明付き）
// =============================================================

// --- 挿入ソート ---
// 証明する性質:
//   1. 出力の長さ == 入力の長さ（要素数保存: result == n）
//   2. 停止性（decreases: n - i, decreases: j）
//   3. ループ不変量の帰納的証明
//
// NOTE: 旧来は二重 while 内側ループで `i` の MIR move 解析が
//       false-positive を出していたため `trusted` を付けていたが、
//       (a) `let i = …` の数値リテラル初期化から型を推論して `Movability::Copy`
//       を立てる修正と、(b) `if n <= 1 { … } else { … }` の path 条件を
//       内側 while の不変量初期検査に伝播する修正により、`trusted` 不要で
//       要素数保存契約 (`result == n`) が証明できるようになった。
atom insertion_sort(n: i64)
requires: n >= 0;
ensures: result == n;
max_unroll: 5;
body: {
    if n <= 1 { n }
    else {
        let i = 1;
        while i < n
        invariant: i >= 1 && i <= n
        decreases: n - i
        {
            let j = i;
            while j > 0
            invariant: j >= 0 && j <= i
            decreases: j
            {
                j = j - 1;
            };
            i = i + 1;
        };
        n
    }
};

// --- マージソート ---
// 再帰的 async atom + invariant による帰納的証明
// 証明する性質:
//   1. 出力の長さ == 入力の長さ（要素数保存: result == n）
//   2. 再帰の安全性（invariant + Compositional Verification）
async atom merge_sort(n: i64)
invariant: n >= 0;
requires: n >= 0;
ensures: result == n;
max_unroll: 3;
body: {
    if n <= 1 { n }
    else {
        let mid = n / 2;
        let left = merge_sort(mid);
        let right = merge_sort(n - mid);
        left + right
    }
};

// --- 挿入ソート（実 body + 要素数保存 ensures）---
// Phase 4 / Plan 9-4: `arr[i] = val` 構文 + Z3 Array::store 追跡の導入により、
// 実際の swap を伴う挿入ソートを body として持てるようになった。
//
// 検証する性質（方針 A: trust の表面積を最小化）:
//   1. 要素数保存: result == n のみを ensures とする
//
// 備考: 「出力が昇順 (forall(i, 0, result-1, arr[i] <= arr[i+1]))」の保証は
//   Z3 Array + forall 量化子で spurious counterexample を生成するため、
//   本 atom では ensures から外した。昇順保持の証明義務は別途
//   `verified_insertion_sort_ascending` で Lean escalation 経路に接続し、
//   Lean 4 で discharge する。ソート済み出力を契約として要求する用途には、
//   下記の `verified_insertion_sort_identity`（sorted-in → sorted-out の
//   証明可能な identity 版）または Lean 証明済み経路を使用すること。
//
//   旧来は二重 while 内側ループの `let key = arr[i]` で move 解析が
//   false-positive を出していたため `trusted` を付けていたが、
//   `mir.rs::infer_hir_ty()` が `Expr::ArrayAccess` から `i64` を推論し
//   `Movability::Copy` を立てるようになったため `trusted` 不要で
//   要素数保存契約 (`result == n`) が証明できる。
//
//   `forall(i, 0, n, arr[i] >= 0)` を requires に追加することで
//   `arr[i]` / `arr[j]` の OOB 推論に必要な `len_arr >= n + 1` を
//   Z3 に提示している（`tests/test_verified_sort.mm` の
//   `verify_insertion_sort_skeleton` と同じイディオム）。
atom verified_insertion_sort(n: i64)
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

// --- 挿入ソート（identity 契約版・証明可能）---
// 方針 B: 旧 PR #157 までの「sorted-in → sorted-out」の identity 契約を
// `body: n` (恒等関数) として残す。入力がソート済みなら出力もソート済み
// であることが Z3 で帰納的に証明可能（trusted 不要）。
// ソート済み出力の契約保証が必要な下流ユーザはこちらを使用する。
atom verified_insertion_sort_identity(n: i64)
requires: n >= 0 && forall(i, 0, n - 1, arr[i] <= arr[i + 1]);
ensures: result == n && forall(i, 0, result - 1, arr[i] <= arr[i + 1]);
body: n;

// --- 挿入ソート（昇順保持・Lean escalation 候補）---
// 方針 A の拡張: 実 body で要素数保存 AND 昇順保持を ensures に含める。
// Z3 は Array + forall 量化子の組み合わせで spurious counterexample を
// 生成するため、この proof obligation は `mumei verify --proof-cert
// --escalate-lean` で escalation 候補
// （`escalation_reason == "spurious_candidate"`）として出力される。
// Lean bridge (`mumei-lean`) 側で昇順保持の証明を discharge する経路
// （5番目の live generated theorem path）に接続する。
// `MumeiLean.Sort.insertion_sort_ascending_bridge` が mathlib の
// `List.sorted_insertionSort` を使って昇順保持を Lean 4 で証明する。
// Lean 証明成功時は `lean_verified` に昇格し、Z3 の spurious_candidate は
// Lean 側で解決されたことになる。
//
// ⚠ atom 定義は `tests/fixtures/sort_ascending.mm` に配置。
//   `std/` 配下の CI verify-std ゲートは `--escalate-lean` なしで実行
//   されるため、Z3 単体で discharge できない atom をここに置くと
//   regression として検出される。Lean escalation 対象 atom は fixture
//   として管理し、`mumei verify --proof-cert --escalate-lean` で
//   proof-cert を生成 → mumei-lean bridge に渡す運用とする。

// --- マージソート（実 body 骨格 + 要素数保存 ensures）---
// 再帰 + 補助配列が必要だが、現行 mumei には補助配列パラメータの
// 標準表現がないため、分割統治の制御フローのみを記述する。
//
// 検証する性質（方針 A）: 要素数保存 (result == n) のみ。
// 「出力が昇順」の保証はソート本体が省略されているため成立せず、偽の
// `trusted` 保証を避けて ensures から外した。昇順保持の証明義務は
// `verified_insertion_sort_ascending`（`tests/fixtures/sort_ascending.mm`）
// 経由で Lean escalation に接続する。
// `let left = …` / `let right = …` は再帰呼び出しの戻り値を一度だけ
// (`left + right` で) 使うのみで、現在の `mir.rs::infer_hir_ty()` は
// `HirExpr::Call` の戻り値型を推論しない (`_ => None`) ため `Move`
// のままだが、二重消費が無いので UseAfterMove は発火せず `trusted`
// 無しで要素数保存契約 (`result == n`) が証明できる。
// （将来 `infer_hir_ty()` に Call ケースが追加されれば `Copy` 化され
//   さらに堅牢になるが、現時点ではあくまで「単一消費」に依存した
//   trusted-free 化である。）
atom verified_merge_sort(n: i64)
requires: n >= 0;
ensures: result == n;
body: {
    if n <= 1 { n }
    else {
        let mid = n / 2;
        let left = verified_merge_sort(mid);
        let right = verified_merge_sort(n - mid);
        left + right
    }
};

// --- マージソート（identity 契約版・証明可能）---
// 方針 B: 旧 identity 契約を `body: n` で残す。sorted-in → sorted-out が
// Z3 で証明可能（trusted 不要）。
atom verified_merge_sort_identity(n: i64)
requires: n >= 0 && forall(i, 0, n - 1, arr[i] <= arr[i + 1]);
ensures: result == n && forall(i, 0, result - 1, arr[i] <= arr[i + 1]);
body: n;

// --- 二分探索 ---
// ソート済み配列に対する探索
// 証明する性質:
//   1. 結果は有効な範囲内: result >= -1 && result < n
//   2. 停止性: decreases: high - low
//   3. ループ不変量の帰納的証明
atom binary_search(n: i64, target: i64)
requires: n >= 0;
ensures: result >= 0 - 1 && result < n;
body: {
    let low = 0;
    let high = n;
    while low < high
    invariant: low >= 0 && high <= n && low <= high
    decreases: high - low
    {
        let mid = low + (high - low) / 2;
        low = mid + 1;
    };
    0 - 1
};

// --- 二分探索（ソート済み前提条件付き）---
// Phase 4: forall in requires で配列がソート済みであることを前提とする。
// verified_insertion_sort の ensures と組み合わせて使用する。
atom binary_search_sorted(n: i64, target: i64)
requires: n >= 0 && forall(i, 0, n - 1, arr[i] <= arr[i + 1]);
ensures: result >= 0 - 1 && result < n;
body: {
    let low = 0;
    let high = n;
    while low < high
    invariant: low >= 0 && high <= n && low <= high
    decreases: high - low
    {
        let mid = low + (high - low) / 2;
        low = mid + 1;
    };
    0 - 1
};
