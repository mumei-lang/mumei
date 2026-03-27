// =============================================================
// std/prelude.mm — Mumei Standard Prelude
// =============================================================
// 全モジュールに自動インポートされる基盤定義。
// 基本トレイト（数学的保証付き）、基本 ADT、
// および将来の動的メモリ管理に向けた抽象インターフェースを提供する。
//
// このファイルはコンパイラが暗黙的にロードする。
// ユーザーが `import "std/prelude"` を書く必要はない。
//
// 注: 基本トレイト (Eq, Ord, Numeric) の impl は
//     コンパイラが i64/u64/f64 に対して自動適用する。
//     law（推移律など）は Z3 上で既知の公理として扱われる。

// =============================================================
// A. 基本トレイト（数学的基盤）
// =============================================================
// Mumei の law 機構により、単なるメソッド定義以上の
// 「数学的保証」が Z3 によって検証される。

// --- Eq: 等価性 ---
// 反射律・対称律を Z3 で保証する。
trait Eq {
    fn eq(a: Self, b: Self) -> bool;
    law reflexive: eq(x, x) == true;
    law symmetric: eq(a, b) => eq(b, a);
}

// --- Ord: 全順序 ---
// 反射律・推移律を Z3 で保証する。
// Eq を暗黙的に前提とする（将来のトレイト継承で明示化予定）。
trait Ord {
    fn leq(a: Self, b: Self) -> bool;
    law reflexive: leq(x, x) == true;
    law transitive: leq(a, b) && leq(b, c) => leq(a, c);
}

// --- Numeric: 算術演算 ---
// 加法の交換律を Z3 で保証する。
// div の第2引数に精緻型制約 `where v != 0` を付与し、
// ゼロ除算を型レベルで排除する。
// Z3 は多相的な演算においても常にゼロ除算の可能性をチェックする。
trait Numeric {
    fn add(a: Self, b: Self) -> Self;
    fn sub(a: Self, b: Self) -> Self;
    fn mul(a: Self, b: Self) -> Self;
    fn div(a: Self, b: Self where v != 0) -> Self;
    law commutative_add: add(a, b) == add(b, a);
}

// =============================================================
// B. 基本 ADT（ジェネリック列挙型・構造体）
// =============================================================

// --- Generic Pair ---
struct Pair<T, U> {
    first: T,
    second: U
}

// --- Generic Option ---
enum Option<T> {
    None,
    Some(T)
}

// --- Generic Result ---
enum Result<T, E> {
    Ok(T),
    Err(E)
}

// --- Generic List (recursive ADT) ---
enum List<T> {
    Nil,
    Cons(T, Self)
}

// =============================================================
// C. コレクション抽象インターフェース（ロードマップ）
// =============================================================
// 動的メモリ管理（alloc）導入前に「インターフェース」として
// 定義しておくことで、将来の実装差し替えを容易にする。
//
// 現時点では固定長配列ベースのコードでも、
// これらのトレイトに準拠して書いておけば、
// alloc 導入後に Vector<T> 等の具体実装に
// 差し替えるだけでロジックの変更が不要になる。

// --- Sequential: 順序付きコレクション ---
// Vector<T> の抽象インターフェース。
// law により長さの非負性を型レベルで保証する。
//
// 将来の alloc 導入時:
//   impl Sequential for Vector<T> { ... }
//   として具体実装を差し込む。
trait Sequential {
    fn seq_len(s: Self) -> i64;
    fn seq_get(s: Self, index: i64) -> i64;
    law non_negative_length: seq_len(x) >= 0;
    law bounds_safe: index >= 0 && index < seq_len(s) => seq_get(s, index) >= 0;
}

// --- Hashable: ハッシュ可能な型 ---
// HashMap<K, V> の Key 制約として使用する。
// 決定性（同じ値は同じハッシュ）を Z3 で保証する。
//
// HashMap<K, V> の具体実装は std/alloc.mm に定義済み。
// Usage:
//   import "std/alloc" as alloc;
//   // map_insert, map_get, map_contains_key, map_remove 等が利用可能
trait Hashable {
    fn hash(a: Self) -> i64;
    law deterministic: hash(x) == hash(x);
}

// =============================================================
// D. 動的メモリ管理（alloc）
// =============================================================
// RawPtr 型、所有権トレイト (Owned)、Vector<T> 構造体、
// および alloc/dealloc/vec_* atom は std/alloc.mm に定義。
//
// 使用方法:
//   import "std/alloc" as alloc;
//
// 詳細は std/alloc.mm を参照。

// =============================================================
// E. Prelude Atoms（基本操作）
// =============================================================

// Option の判定: Some(tag=1) なら 1, None(tag=0) なら 0
atom prelude_is_some(opt: i64)
    requires: opt >= 0 && opt <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match opt {
            1 => 1,
            _ => 0
        }
    }

// Option の判定: None(tag=0) なら 1, Some(tag=1) なら 0
atom prelude_is_none(opt: i64)
    requires: opt >= 0 && opt <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match opt {
            0 => 1,
            _ => 0
        }
    }

// Result の判定: Ok(tag=0) なら 1, Err(tag=1) なら 0
atom prelude_is_ok(res: i64)
    requires: res >= 0 && res <= 1;
    ensures: result >= 0 && result <= 1;
    body: {
        match res {
            0 => 1,
            _ => 0
        }
    }
