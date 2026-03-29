// =============================================================
// tests/test_libc_contracts_error.mm — libc Contract Violation Tests
// =============================================================
// This file is expected to FAIL verification.
// Each atom deliberately violates a safe_* wrapper's requires clause
// to confirm that Z3 rejects invalid call sites.
//
// Usage: mumei check tests/test_libc_contracts_error.mm
// Expected: verification errors for all atoms below.

import "std/libc" as libc;

// --- memcpy: n > dst_size (buffer overrun) ---
// safe_memcpy requires: n >= 0 && dst_size >= n && src_size >= n
// Here dst_size=50 < n=100 — should fail.
atom bad_memcpy_dst_overflow()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_memcpy(50, 100, 100);

// --- malloc: size == 0 ---
// safe_malloc requires: size > 0
// Here size=0 — should fail.
atom bad_malloc_zero()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_malloc(0);

// --- strlen: negative length ---
// safe_strlen requires: s_len >= 0
// Here s_len=-1 — should fail.
atom bad_strlen_negative()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_strlen(-1);

// --- free: negative pointer ---
// safe_free requires: ptr >= 0
// Here ptr=-1 — should fail.
atom bad_free_negative()
    requires: true;
    ensures: result == 0;
    body: libc::safe_free(-1);

// --- snprintf: n > buf_size ---
// safe_snprintf requires: buf_size > 0 && n > 0 && n <= buf_size
// Here n=512 > buf_size=256 — should fail.
atom bad_snprintf_overflow()
    requires: true;
    ensures: result >= 0;
    body: libc::safe_snprintf(256, 512);
