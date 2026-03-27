// E2E test: Verified collection operations
// Tests cross-atom contract composition: callee requires are checked
// against caller context, callee ensures propagate to caller.
import "std/alloc" as alloc;

// Test: vec_push followed by vec_get is safe
// Verifies: push(0, cap) satisfies vec_get requires (len > 0)
atom test_push_then_get(cap: i64)
    requires: cap > 1;
    ensures: result >= 0;
    body: {
        let len = alloc.vec_push(0, cap);
        alloc.vec_get(len, 0)
    }

// Test: vec_insert after vec_push — both operations compose safely
// Verifies: push ensures (result >= 0 && result <= cap) satisfies insert requires
atom test_insert_length(len: i64, cap: i64)
    requires: len >= 0 && cap > 0 && len + 1 < cap;
    ensures: result >= 0;
    body: {
        let after_push = alloc.vec_push(len, cap);
        alloc.vec_insert(after_push, cap, 0)
    }

// Test: vec_remove after push — composition is safe
// Verifies: push ensures (result >= 0) satisfies remove requires (len > 0)
atom test_push_remove_identity(len: i64, cap: i64)
    requires: len >= 0 && cap > 0 && len < cap;
    ensures: result >= 0;
    body: {
        let after_push = alloc.vec_push(len, cap);
        alloc.vec_remove(after_push, 0)
    }

// Test: vec_slice bounds safety
// Verifies: slice requires (start >= 0 && end >= start && end <= len) hold
atom test_slice_length(len: i64)
    requires: len >= 4;
    ensures: result >= 0;
    body: {
        alloc.vec_slice(len, 1, 3)
    }
