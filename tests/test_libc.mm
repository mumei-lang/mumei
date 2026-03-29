// =============================================================
// tests/test_libc.mm — Verified C Library Wrapper Tests
// =============================================================
// safe_* ラッパーを呼び出す atom を定義し、
// 呼び出し元の requires が Z3 により検証されることをテストする。
//
// Usage: mumei verify tests/test_libc.mm
// Expected: all atoms pass verification.

import "std/libc" as libc;

// --- memcpy ---
atom test_safe_memcpy()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memcpy(100, 100, 50);

// --- memmove ---
atom test_safe_memmove()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memmove(200, 200, 100);

// --- memset ---
atom test_safe_memset()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memset(128, 0, 128);

// --- strlen ---
atom test_safe_strlen()
    requires: true;
    ensures: result >= 0 && result < 256;
    body: libc::safe_strlen(256);

// --- malloc ---
atom test_safe_malloc()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_malloc(1024);

// --- free ---
atom test_safe_free()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_free(42);

// --- calloc ---
atom test_safe_calloc()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_calloc(10, 64);

// --- realloc ---
atom test_safe_realloc()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_realloc(0, 0, 256);

// --- snprintf ---
atom test_safe_snprintf()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_snprintf(256, 128);

// --- 組み合わせテスト ---
// malloc → memcpy パイプライン
atom test_alloc_copy_pipeline(src_size: i64, n: i64)
    requires: src_size >= 0 && n >= 0 && n <= src_size && n <= 1024;
    ensures: result >= 0;
    body: {
        let buf_size = n + 1;
        libc::safe_memcpy(buf_size, src_size, n)
    };

// calloc → memset パイプライン
// calloc で確保 → memset でフィル（確保失敗時はガード）
atom test_calloc_memset_pipeline()
    requires: true;
    ensures: result >= -1;
    body: {
        let ptr = libc::safe_calloc(4, 64);
        if ptr >= 0 then
            libc::safe_memset(256, 42, 128)
        else ptr
    };

// memmove with exact buffer sizes
atom test_memmove_exact()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memmove(64, 64, 64);
