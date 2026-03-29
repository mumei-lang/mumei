// =============================================================
// Tests for std/libc.mm — Verified C Library Wrappers
// =============================================================
// safe_* ラッパーを呼び出す atom を定義し、
// 呼び出し元の requires が Z3 により検証されることをテストする。
//
// Usage: mumei verify tests/test_libc_contracts.mm
// Expected: all atoms pass verification.

import "std/libc" as libc;

// --- memcpy ---
// 既知のバッファサイズでコピーする（安全）
atom test_memcpy_safe()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memcpy(100, 100, 50);

// --- memmove ---
// オーバーラップ許容の安全なメモリ移動
atom test_memmove_safe()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memmove(200, 200, 100);

// --- memset ---
// バッファサイズと値の範囲を満たす呼び出し（安全）
atom test_memset_safe()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memset(128, 0, 128);

// --- strlen ---
// 正のバッファサイズで呼ぶ（安全）
atom test_strlen_safe()
    requires: true;
    ensures: result >= 0 && result < 256;
    body: libc::safe_strlen(256);

// --- malloc ---
// 正のサイズで呼ぶ（安全）
atom test_malloc_safe()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_malloc(64);

// --- free ---
// 非負ポインタで呼ぶ（安全）
atom test_free_safe()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_free(1024);

// --- calloc ---
// 正の count と size で呼ぶ（安全）
atom test_calloc_safe()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_calloc(10, 64);

// --- realloc ---
// 有効なポインタと正の新サイズで呼ぶ（安全）
atom test_realloc_safe()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_realloc(0, 0, 128);

// --- snprintf ---
// バッファサイズ制約を満たす呼び出し（安全）
atom test_snprintf_safe()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_snprintf(256, 128);

// --- 組み合わせテスト ---
// malloc → memcpy の安全な組み合わせ
atom test_alloc_and_copy(src_size: i64, n: i64)
    requires: src_size >= 0 && n >= 0 && n <= src_size && n <= 1024;
    ensures: result >= 0;
    body: {
        let buf_size = n + 1;
        libc::safe_memcpy(buf_size, src_size, n)
    };

// calloc → memset → free の安全な組み合わせ
// calloc で確保 → memset でフィル → free で解放
// calloc/malloc は -1 を返す可能性があるためガード付き
atom test_calloc_memset_free()
    requires: true;
    ensures: result >= -1;
    body: {
        let ptr = libc::safe_calloc(4, 64);
        if ptr >= 0 then {
            let filled = libc::safe_memset(256, 42, 128);
            libc::safe_free(ptr)
        } else ptr
    };
