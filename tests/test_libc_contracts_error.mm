// =============================================================
// tests/test_libc_contracts_error.mm — libc Contract Violation Tests
// =============================================================
// This file is expected to FAIL verification.
// Each atom deliberately violates a safe_* wrapper's requires clause
// to confirm that Z3 rejects invalid call sites (precondition_violated).
//
// Usage: mumei verify tests/test_libc_contracts_error.mm
// Expected: verification errors for all atoms below.

import "std/libc" as libc;

// --- memcpy: n > dst_size (buffer overrun) ---
// safe_memcpy requires: n >= 0 && dst_size >= n && src_size >= n
// Here dst_size=50 < n=100 — should fail.
atom bad_memcpy_dst_overflow()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memcpy(50, 100, 100);

// --- memmove: n > src_size (source buffer overrun) ---
// safe_memmove requires: n >= 0 && dst_size >= n && src_size >= n
// Here src_size=30 < n=50 — should fail.
atom bad_memmove_src_overflow()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memmove(100, 30, 50);

// --- memset: value > 255 ---
// safe_memset requires: value >= 0 && value <= 255
// Here value=256 — should fail.
atom bad_memset_value_overflow()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memset(64, 256, 32);

// --- malloc: size == 0 ---
// safe_malloc requires: size > 0
// Here size=0 — should fail.
atom bad_malloc_zero()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_malloc(0);

// --- strlen: zero buffer size ---
// safe_strlen requires: buf_size > 0
// Here buf_size=0 — should fail.
atom bad_strlen_zero()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_strlen(0);

// --- free: negative pointer ---
// safe_free requires: ptr >= 0
// Here ptr=-1 — should fail.
atom bad_free_negative()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_free(-1);

// --- calloc: count == 0 ---
// safe_calloc requires: count > 0 && size > 0
// Here count=0 — should fail.
atom bad_calloc_zero_count()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_calloc(0, 64);

// --- realloc: negative new_size ---
// safe_realloc requires: new_size > 0
// Here new_size=0 — should fail.
atom bad_realloc_zero_size()
    requires: true;
    ensures: result >= -1;
    body: libc::safe_realloc(0, 64, 0);

// --- snprintf: n > buf_size ---
// safe_snprintf requires: buf_size > 0 && n > 0 && n <= buf_size
// Here n=512 > buf_size=256 — should fail.
atom bad_snprintf_overflow()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_snprintf(256, 512);
