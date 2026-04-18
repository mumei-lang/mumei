// =============================================================
// std/core.mm — Mumei Core Axioms (最小の真理セット)
// =============================================================
// プロジェクト全体で共有すべき基礎的な型定義と安全な操作。
// 全ての std モジュールはこのモジュールの型と契約を
// 「公理（Axiom）」として利用できる。
//
// Usage:
//   import "std/core" as core;
//
// 設計方針:
// - std/prelude.mm (暗黙ロード) と std/contracts.mm (汎用精緻型) より
//   さらに下層に位置する、最小の数学的基盤。
// - 全ての atom に requires/ensures を付与し、Z3 で完全検証する。
// - `trusted atom` は使用しない（公理として信頼するのは型と契約のみ）。
//
// =============================================================
// グローバル・インバリアント（Global Invariants）
// =============================================================
// 1. 全てのサイズ型（Size）は非負である。
//      Size >= 0 ── 要素数・長さは数学的に非負。
// 2. 全てのインデックスは非負である。
//      Index >= 0 ── 配列の先頭要素は 0 から始まる。
// 3. 全ての境界付きインデックスは非負で、使用側が上限を明示する。
//      BoundedIndex >= 0 && BoundedIndex < N (呼び出し時に N を契約で要求)
// 4. 全ての配列操作は境界チェック済みである。
//      std/alloc.mm の vec_get/vec_set は (index < vec_len) を requires で強制する。
// 5. ゼロ除算は型レベルで禁止される。
//      NonZero 型（v != 0）を除算の分母に要求する。
//
// これらのインバリアントは std の各モジュールが暗黙的に前提とする
// 「公理」であり、Z3 は全ての検証でこれらを信頼する。

// =============================================================
// A. 基本精緻型（Core Refinement Types）
// =============================================================

// --- Size ---
// 全てのサイズ・長さを表す非負整数。
// 例: コレクションの要素数、バッファの容量、文字列長。
type Size = i64 where v >= 0;

// --- Index ---
// 配列・スライスの有効インデックス。
// 上限は各使用コンテキストで渡される（Size との組み合わせで境界を表現）。
type Index = i64 where v >= 0;

// --- NonZero ---
// ゼロを除く整数。除算の分母や非ゼロ前提の計算に使用。
// Numeric トレイトの div と同じ制約を型レベルで表現する。
type NonZero = i64 where v != 0;

// --- BoundedIndex ---
// 境界付きインデックス。現時点では v >= 0 のみ保証し、
// 上限は使用時に safe_narrow などで明示的に絞り込む。
type BoundedIndex = i64 where v >= 0;

// =============================================================
// B. 安全な型変換 Atom（Safe Conversion Axioms）
// =============================================================
// 任意の i64 値を、精緻型制約を満たす値へ安全に変換する基礎 atom 群。
// 全てが純粋関数であり、副作用なし・契約完全検証。

// --- safe_to_non_negative ---
// 負の値を 0 にクランプする。`i64 -> Size` / `i64 -> Index` への変換基盤。
// ensures: result >= 0 を常に保証。
atom safe_to_non_negative(x: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        if x >= 0 { x } else { 0 }
    };

// --- safe_narrow ---
// 値を [min_val, max_val] の範囲に絞り込む。
// std/contracts.mm の clamp より基礎的な位置づけで、
// 任意の精緻型への安全な変換基盤として使用する。
atom safe_narrow(x: i64, min_val: i64, max_val: i64)
    requires: min_val <= max_val;
    ensures: result >= min_val && result <= max_val;
    body: {
        if x < min_val { min_val }
        else { if x > max_val { max_val } else { x } }
    };

// --- checked_add ---
// オーバーフロー検出付き飽和加算。
// 両オペランドが非負かつ max_val 以下のとき、合計が max_val を超えたら max_val を返す。
atom checked_add(a: i64, b: i64, max_val: i64)
    requires: a >= 0 && b >= 0 && max_val >= 0 && a <= max_val && b <= max_val;
    ensures: result >= 0 && result <= max_val;
    body: {
        if a + b <= max_val { a + b } else { max_val }
    };

// --- checked_sub ---
// アンダーフロー検出付き飽和減算。
// 両オペランドが非負のとき、a < b なら 0 を返す。
// 常に result >= 0 を保証し、Size 型の不変量を破らない。
atom checked_sub(a: i64, b: i64)
    requires: a >= 0 && b >= 0;
    ensures: result >= 0;
    body: {
        if a >= b { a - b } else { 0 }
    };

// --- checked_mul ---
// オーバーフロー検出付き飽和乗算。
// 両オペランドが非負のとき、積が max_val を超えたら max_val を返す。
// 乗算は非線形演算のため、Z3 は部分的に推論する（a == 0 / b == 0 で早期終了）。
atom checked_mul(a: i64, b: i64, max_val: i64)
    requires: a >= 0 && b >= 0 && max_val >= 0;
    ensures: result >= 0 && result <= max_val;
    body: {
        if a == 0 { 0 }
        else {
            if b == 0 { 0 }
            else {
                if a * b <= max_val { a * b } else { max_val }
            }
        }
    };

// =============================================================
// C. 基本比較・等価性 Atom（Core Comparison Axioms）
// =============================================================

// --- equals ---
// 等価判定（0 = false, 1 = true）。
// Eq トレイトの実行時投影。Z3 上では a == b と同値。
atom equals(a: i64, b: i64)
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        if a == b { 1 } else { 0 }
    };

// --- compare ---
// 三値比較（-1 = a < b, 0 = a == b, 1 = a > b）。
// Ord トレイトの実行時投影。
atom compare(a: i64, b: i64)
    requires: true;
    ensures: result >= -1 && result <= 1;
    body: {
        if a < b { 0 - 1 } else { if a > b { 1 } else { 0 } }
    };

// --- min ---
// 2つの値の最小値。
atom min(a: i64, b: i64)
    requires: true;
    ensures: result <= a && result <= b;
    body: {
        if a <= b { a } else { b }
    };

// --- max ---
// 2つの値の最大値。
atom max(a: i64, b: i64)
    requires: true;
    ensures: result >= a && result >= b;
    body: {
        if a >= b { a } else { b }
    };
