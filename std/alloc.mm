// =============================================================
// std/alloc.mm — Mumei Dynamic Memory Management
// =============================================================
// 動的メモリ管理の基盤モジュール。
// RawPtr 型、所有権トレイト、Vector 構造体、
// および alloc/dealloc の atom を提供する。
//
// Usage:
//   import "std/alloc" as alloc;
// --- STEP 1: RawPtr — 生ポインタの精緻型表現 ---
type RawPtr = i64 where v >= 0;
type NullablePtr = i64 where v >= -1;
// --- STEP 2: 所有権トレイト（Linear Types の近似）---
trait Owned {
    fn is_alive(a: Self) -> bool;
    fn consume(a: Self) -> Self;
    law alive_before_consume: is_alive(x) == true;
}
// --- STEP 3: Vector<T> 構造体定義 ---
struct Vector<T> {
    ptr: i64 where v >= 0,
    len: i64 where v >= 0,
    cap: i64 where v > 0
}
// --- メモリ確保・解放 ---
atom alloc_raw(size: i64)
    requires: size > 0;
    ensures: result >= -1;
    body: {
        if size > 0 { 0 } else { -1 }
    }
atom dealloc_raw(ptr: i64)
    requires: ptr >= 0;
    ensures: result >= 0;
    body: { 0 }
// --- Vector 操作 ---
atom vec_new(initial_cap: i64)
    requires: initial_cap > 0;
    ensures: result >= 0;
    body: { 0 }
atom vec_push(vec_len: i64, vec_cap: i64)
    requires: vec_len >= 0 && vec_cap > 0 && vec_len < vec_cap;
    ensures: result >= 0 && result <= vec_cap && result == vec_len + 1;
    body: { vec_len + 1 }
atom vec_get(vec_len: i64, index: i64)
    requires: vec_len > 0 && index >= 0 && index < vec_len;
    ensures: result >= 0;
    body: { index }
atom vec_len(len: i64)
    requires: len >= 0;
    ensures: result >= 0 && result == len;
    body: { len }
atom vec_is_empty(len: i64)
    requires: len >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        if len == 0 { 1 } else { 0 }
    }
atom vec_grow(old_cap: i64, new_cap: i64)
    requires: old_cap > 0 && new_cap > old_cap;
    ensures: result > old_cap;
    body: { new_cap }
atom vec_drop(vec_len: i64, vec_ptr: i64)
    requires: vec_len >= 0 && vec_ptr >= 0;
    ensures: result >= 0;
    body: { 0 }
atom vec_push_safe(vec_len: i64, vec_cap: i64)
    requires: vec_len >= 0 && vec_cap > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        if vec_len < vec_cap { 0 } else { 1 }
    }

// 境界チェック付き要素設定
// NOTE: value >= 0 は vec_get の ensures: result >= 0 との整合性のために必要。
// ベクター要素は非負整数に制限される（現行の検証フレームワークの設計上の制約）。
// 将来、任意の i64 値をサポートする場合は vec_get の ensures も同時に緩和すること。
atom vec_set(vec_len: i64, index: i64, value: i64)
    requires: vec_len > 0 && index >= 0 && index < vec_len && value >= 0;
    ensures: result >= 0;
    body: { value }

// 2つのインデックスの要素を交換（両方の境界チェック）
atom vec_swap(vec_len: i64, i: i64, j: i64)
    requires: vec_len > 0 && i >= 0 && i < vec_len && j >= 0 && j < vec_len;
    ensures: result >= 0;
    body: { 0 }

// スライス操作（範囲チェック付き）
atom vec_slice(vec_len: i64, start: i64, end: i64)
    requires: vec_len >= 0 && start >= 0 && end >= start && end <= vec_len;
    ensures: result >= 0 && result == end - start;
    body: { end - start }

// 指定位置への挿入（境界 + 容量チェック）
atom vec_insert(vec_len: i64, vec_cap: i64, index: i64)
    requires: vec_len >= 0 && vec_cap > 0 && vec_len < vec_cap && index >= 0 && index <= vec_len;
    ensures: result >= 0 && result == vec_len + 1;
    body: { vec_len + 1 }

// 指定位置の要素削除（境界チェック付き）
atom vec_remove(vec_len: i64, index: i64)
    requires: vec_len > 0 && index >= 0 && index < vec_len;
    ensures: result >= 0 && result == vec_len - 1;
    body: { vec_len - 1 }

// =============================================================
// HashMap<K, V> — ハッシュマップの構造体定義と操作
// =============================================================
// キー制約: K は Hashable + Eq を満たす必要がある（prelude で定義済み）。
// 内部はオープンアドレス法に基づくバケット配列として設計。
//
// 精緻型制約による不変条件（Z3 で常に検証）:
//   - buckets >= 0（有効なバケットポインタ）
//   - size >= 0（要素数は非負）
//   - capacity > 0（容量は正）
//   - size <= capacity（暗黙: insert の requires で保証）
//
// Usage:
//   import "std/alloc" as alloc;
//
//   atom example(cap: i64)
//       requires: cap > 0;
//       ensures: result >= 0;
//       body: {
//           let size = map_new(cap);
//           let new_size = map_insert(size, cap);
//           new_size
//       };

struct HashMap<K, V> {
    buckets: i64 where v >= 0,
    size: i64 where v >= 0,
    capacity: i64 where v > 0
}

// --- HashMap 操作 Atom ---

// HashMap 新規作成: 初期容量を指定して空のマップを生成
// ensures: size == 0（空のマップ）
atom map_new(initial_capacity: i64)
    requires: initial_capacity > 0;
    ensures: result >= 0;
    body: { 0 }

// HashMap への要素挿入: size < capacity の場合のみ許可
// 同一キーが既に存在する場合は上書き（size は増えない可能性がある）
// ensures: result <= size + 1（新規挿入なら +1、上書きなら同じ）
atom map_insert(map_size: i64, map_capacity: i64)
    requires: map_size >= 0 && map_capacity > 0 && map_size < map_capacity;
    ensures: result >= 0 && result <= map_size + 1;
    body: { map_size + 1 }

// HashMap からの要素取得: キーに対応する値の存在を Result で返す
// 0 = 見つかった（Ok）, 1 = 見つからない（Err）
atom map_get(map_size: i64, key_hash: i64)
    requires: map_size >= 0 && key_hash >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        if map_size > 0 { 0 } else { 1 }
    }

// HashMap にキーが存在するかチェック
// 1 = 存在する, 0 = 存在しない
atom map_contains_key(map_size: i64, key_hash: i64)
    requires: map_size >= 0 && key_hash >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        if map_size > 0 { 1 } else { 0 }
    }

// HashMap からの要素削除: キーが存在すれば削除して size - 1 を返す
// キーが存在しなければ size をそのまま返す
atom map_remove(map_size: i64, key_hash: i64)
    requires: map_size >= 0 && key_hash >= 0;
    ensures: result >= 0 && result <= map_size;
    body: {
        if map_size > 0 { map_size - 1 } else { 0 }
    }

// HashMap のサイズ取得
atom map_size(size: i64)
    requires: size >= 0;
    ensures: result >= 0 && result == size;
    body: { size }

// HashMap が空かどうか判定
atom map_is_empty(size: i64)
    requires: size >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        if size == 0 { 1 } else { 0 }
    }

// HashMap の容量拡張（リハッシュ）
// 新しい容量は現在の容量より大きい必要がある
atom map_rehash(old_capacity: i64, new_capacity: i64)
    requires: old_capacity > 0 && new_capacity > old_capacity;
    ensures: result > old_capacity;
    body: { new_capacity }

// HashMap の解放
atom map_drop(map_size: i64, map_buckets: i64)
    requires: map_size >= 0 && map_buckets >= 0;
    ensures: result >= 0;
    body: { 0 }

// 安全な挿入: 容量チェック付き（Result 型: 0=Ok, 1=Err=容量不足）
atom map_insert_safe(map_size: i64, map_capacity: i64)
    requires: map_size >= 0 && map_capacity > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        if map_size < map_capacity { 0 } else { 1 }
    }

// 負荷率チェック: size が capacity の 75% を超えたら 1（リハッシュ推奨）
// 整数演算で近似: size * 4 > capacity * 3
atom map_should_rehash(map_size: i64, map_capacity: i64)
    requires: map_size >= 0 && map_capacity > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        if map_size * 4 > map_capacity * 3 { 1 } else { 0 }
    }
