// =============================================================
// Path Safety: E2E Verification Tests
// =============================================================
// Tests for compile-time directory traversal prevention using
// parameterized effects with compound constraints.
//
// Usage:
//   mumei check tests/test_path_safety.mm

// Security policy effect: only /tmp/ paths, no ".." allowed
effect SafeFileRead(path: Str) where starts_with(path, "/tmp/") && not_contains(path, "..");

// --- Test 1: Safe read with constrained user_id ---
// user_id is required to not contain "..", so the concatenated path
// satisfies both starts_with("/tmp/") and not_contains("..").
atom test_safe_read(user_id: Str)
    effects: [SafeFileRead(path)]
    requires: not_contains(user_id, "..") && not_contains(user_id, "\0");
    ensures: result >= 0;
    body: {
        let path = "/tmp/" + user_id + "/log.txt";
        perform SafeFileRead.read(path);
        1
    }

// --- Test 2: Safe read with literal path ---
// A fully literal path trivially satisfies the constraint.
atom test_literal_path()
    effects: [SafeFileRead(path)]
    requires: true;
    ensures: result >= 0;
    body: {
        let path = "/tmp/data/log.txt";
        perform SafeFileRead.read(path);
        1
    }

// --- Test 3: Safe read with constrained prefix ---
// Demonstrates that concat preserves the /tmp/ prefix.
atom test_concat_prefix(filename: Str)
    effects: [SafeFileRead(path)]
    requires: not_contains(filename, "..") && not_contains(filename, "/");
    ensures: result >= 0;
    body: {
        let path = "/tmp/" + filename;
        perform SafeFileRead.read(path);
        1
    }
