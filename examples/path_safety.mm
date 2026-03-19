// examples/path_safety.mm
// Compile-time directory traversal prevention demo
//
// Usage:
//   mumei check examples/path_safety.mm
//
// Expected: safe_read passes verification, unsafe_read fails

// Security policy effect: only /tmp/ paths, no ".." allowed
effect SafeFileRead(path: Str) where starts_with(path, "/tmp/") && not_contains(path, "..");

// SAFE: user_id is constrained to not contain ".."
atom safe_read(user_id: Str)
    effects: [SafeFileRead(path)]
    requires: not_contains(user_id, "..") && not_contains(user_id, "\0");
    ensures: result >= 0;
    body: {
        let path = "/tmp/" + user_id + "/log.txt";
        perform SafeFileRead.read(path);
        1
    }

// UNSAFE: user_id has no constraints — should cause compile error
atom unsafe_read(user_id: Str)
    effects: [SafeFileRead(path)]
    requires: true;
    ensures: result >= 0;
    body: {
        let path = "/tmp/" + user_id + "/log.txt";
        perform SafeFileRead.read(path);
        1
    }
