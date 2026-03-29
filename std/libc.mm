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
// Usage:
//   import "std/libc" as libc;

// =============================================================
// Extern C declarations with contracts
// =============================================================

extern "C" {
    fn memcpy(dst_size: i64, src_size: i64, n: i64) -> i64
        requires: n >= 0 && dst_size >= n && src_size >= n;
        ensures: result >= 0;

    fn strlen(s_len: i64) -> i64
        requires: s_len >= 0;
        ensures: result >= 0 && result <= s_len;

    fn malloc(size: i64) -> i64
        requires: size > 0;
        ensures: result >= 0;

    fn free(ptr: i64) -> i64
        requires: ptr >= 0;
        ensures: result == 0;

    fn snprintf(buf_size: i64, n: i64) -> i64
        requires: buf_size > 0 && n > 0 && n <= buf_size;
        ensures: result >= 0 && result < n;
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

// safe_strlen: 非負の文字列長を受けて strlen を呼ぶ
// ensures: 結果は 0 以上かつ入力長以下
atom safe_strlen(s_len: i64)
    requires: s_len >= 0;
    ensures: result >= 0 && result <= s_len;
    body: strlen(s_len);

// safe_malloc: 正のサイズで malloc を呼ぶ
// ensures: 非負のポインタ（0 = NULL も許容）
atom safe_malloc(size: i64)
    requires: size > 0;
    ensures: result >= 0;
    body: malloc(size);

// safe_free: 非負のポインタを free する
// ensures: 常に 0 を返す
atom safe_free(ptr: i64)
    requires: ptr >= 0;
    ensures: result == 0;
    body: free(ptr);

// safe_snprintf: バッファサイズ制約を検証した上で snprintf を呼ぶ
// ensures: 書き込みバイト数は 0 以上かつ n 未満
atom safe_snprintf(buf_size: i64, n: i64)
    requires: buf_size > 0 && n > 0 && n <= buf_size;
    ensures: result >= 0 && result < n;
    body: snprintf(buf_size, n);
