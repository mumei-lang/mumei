// =============================================================
// Tests for std/libc.mm — Verified C Library Wrappers
// =============================================================
// safe_* ラッパーを呼び出す atom を定義し、
// 呼び出し元の requires が Z3 により検証されることをテストする。

import "std/libc" as libc;

// --- memcpy ---
// 既知のバッファサイズでコピーする（安全）
atom test_memcpy_safe()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memcpy(100, 100, 50);

// --- strlen ---
// 既知の非負文字列長で呼ぶ（安全）
atom test_strlen_safe()
    requires: true;
    ensures: result >= 0 && result <= 256;
    body: libc::safe_strlen(256);

// --- malloc ---
// 正のサイズで呼ぶ（安全）
atom test_malloc_safe()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_malloc(64);

// --- free ---
// 非負ポインタで呼ぶ（安全）
atom test_free_safe()
    requires: true;
    ensures: result == 0;
    body: libc::safe_free(1024);

// --- snprintf ---
// バッファサイズ制約を満たす呼び出し（安全）
atom test_snprintf_safe()
    requires: true;
    ensures: result >= 0 && result < 128;
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
