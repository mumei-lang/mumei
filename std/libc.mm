// =============================================================
// Mumei Standard Library: libc — Verified C Library Wrappers
// =============================================================
// C 標準ライブラリ関数を安全に呼び出すための検証済みラッパー。
// extern "C" ブロックで C 関数の契約を宣言し、
// ラッパー atom で追加の安全性チェックを行う。
//
// 各 extern 関数は trusted atom として扱われ、
// 呼び出し元で requires が Z3 により検証される。
//
// パラメータは mumei の抽象的なサイズ表現を使用
// （実際のポインタではなくサイズを i64 で表現）。
// これは mumei の型システムの制約に合わせた設計。
//
// Usage:
//   import "std/libc" as libc;
//   let ptr = libc::safe_malloc(1024);

// =============================================================
// Extern C declarations with Verified FFI Contracts
// =============================================================

extern "C" {
    fn memcpy(dst_size: i64, src_size: i64, n: i64) -> i64
        requires: n >= 0 && dst_size >= n && src_size >= n;
        ensures: result >= 0;

    fn memmove(dst_size: i64, src_size: i64, n: i64) -> i64
        requires: n >= 0 && dst_size >= n && src_size >= n;
        ensures: result >= 0;

    fn memset(buf_size: i64, value: i64, n: i64) -> i64
        requires: n >= 0 && buf_size >= n && value >= 0 && value <= 255;
        ensures: result >= 0;

    fn strlen(buf_size: i64) -> i64
        requires: buf_size > 0;
        ensures: result >= 0 && result < buf_size;

    fn malloc(size: i64) -> i64
        requires: size > 0;
        ensures: result >= -1;

    fn free(ptr: i64) -> i64
        requires: ptr >= 0;
        ensures: result >= 0;

    fn calloc(count: i64, size: i64) -> i64
        requires: count > 0 && size > 0;
        ensures: result >= -1;

    fn realloc(ptr: i64, old_size: i64, new_size: i64) -> i64
        requires: ptr >= 0 && old_size >= 0 && new_size > 0;
        ensures: result >= -1;

    fn snprintf(buf_size: i64, n: i64) -> i64
        requires: buf_size > 0 && n > 0 && n <= buf_size;
        ensures: result >= 0;
}

// =============================================================
// Safe wrapper atoms
// =============================================================
// 各ラッパーは extern 関数の requires を満たすことを
// Z3 が呼び出し元で証明する。

// safe_memcpy: バッファサイズを検証した上で memcpy を呼ぶ
// requires: コピーサイズが両バッファ以内
// ensures: 非負の結果を返す
atom safe_memcpy(dst_size: i64, src_size: i64, n: i64)
    requires: n >= 0 && dst_size >= n && src_size >= n;
    ensures: result >= 0;
    body: memcpy(dst_size, src_size, n);

// safe_memmove: オーバーラップ許容の安全なメモリ移動
// requires: 移動サイズが両バッファ以内
// ensures: 非負の結果を返す
atom safe_memmove(dst_size: i64, src_size: i64, n: i64)
    requires: n >= 0 && dst_size >= n && src_size >= n;
    ensures: result >= 0;
    body: memmove(dst_size, src_size, n);

// safe_memset: バッファサイズと値の範囲を検証した上で memset を呼ぶ
// requires: 設定サイズがバッファ以内 && 値が 0-255
// ensures: 非負の結果を返す
atom safe_memset(buf_size: i64, value: i64, n: i64)
    requires: n >= 0 && buf_size >= n && value >= 0 && value <= 255;
    ensures: result >= 0;
    body: memset(buf_size, value, n);

// safe_strlen: 正のバッファサイズを受けて strlen を呼ぶ
// ensures: 結果は 0 以上かつバッファサイズ未満
atom safe_strlen(buf_size: i64)
    requires: buf_size > 0;
    ensures: result >= 0 && result < buf_size;
    body: strlen(buf_size);

// safe_malloc: 正のサイズで malloc を呼ぶ
// ensures: 結果は -1 以上（-1 = 確保失敗）
atom safe_malloc(size: i64)
    requires: size > 0;
    ensures: result >= -1;
    body: malloc(size);

// safe_free: 非負のポインタを free する
// ensures: 非負の結果を返す
atom safe_free(ptr: i64)
    requires: ptr >= 0;
    ensures: result >= 0;
    body: free(ptr);

// safe_calloc: ゼロ初期化メモリを確保
// requires: count > 0 && size > 0
// ensures: 結果は -1 以上（-1 = 確保失敗）
atom safe_calloc(count: i64, size: i64)
    requires: count > 0 && size > 0;
    ensures: result >= -1;
    body: calloc(count, size);

// safe_realloc: メモリ再確保
// requires: 有効なポインタと正の新サイズ
// ensures: 結果は -1 以上（-1 = 確保失敗）
atom safe_realloc(ptr: i64, old_size: i64, new_size: i64)
    requires: ptr >= 0 && old_size >= 0 && new_size > 0;
    ensures: result >= -1;
    body: realloc(ptr, old_size, new_size);

// safe_snprintf: バッファサイズ制約を検証した上で snprintf を呼ぶ
// ensures: 書き込みバイト数は 0 以上
atom safe_snprintf(buf_size: i64, n: i64)
    requires: buf_size > 0 && n > 0 && n <= buf_size;
    ensures: result >= 0;
    body: snprintf(buf_size, n);
