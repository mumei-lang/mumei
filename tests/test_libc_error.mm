// =============================================================
// tests/test_libc_error.mm — libc Contract Violation Tests
// =============================================================
// This file is expected to FAIL verification.
// Each atom deliberately violates a safe_* wrapper's requires clause
// to confirm that Z3 detects precondition_violated.
//
// Usage: mumei verify tests/test_libc_error.mm
// Expected: verification errors (precondition_violated) for all atoms.

import "std/libc" as libc;

// --- memcpy: dst_size < n (buffer overrun) ---
atom test_memcpy_overflow()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memcpy(10, 100, 50);

// --- memmove: src_size < n (source overrun) ---
atom test_memmove_overflow()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memmove(100, 20, 50);

// --- memset: value out of byte range ---
atom test_memset_bad_value()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memset(64, 300, 32);

// --- memset: n > buf_size ---
atom test_memset_overflow()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memset(10, 0, 20);

// --- strlen: zero buffer size ---
atom test_strlen_zero()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_strlen(0);

// --- malloc: size == 0 ---
atom test_malloc_zero()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_malloc(0);

// --- free: negative pointer ---
atom test_free_negative()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_free(-1);

// --- calloc: size == 0 ---
atom test_calloc_zero_size()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_calloc(10, 0);

// --- realloc: new_size == 0 ---
atom test_realloc_zero_newsize()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_realloc(0, 64, 0);

// --- snprintf: n > buf_size ---
atom test_snprintf_overflow()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_snprintf(64, 128);
