// =============================================================
// std/file.mm — Mumei Standard Library: File I/O Primitives
// =============================================================
// Provides verified file read/write operations over opaque i64
// handles (the FFI layer, not this module, owns any path policy).
// `safe_read_file`/`safe_*` variants take `Str` paths and do enforce
// path-prefix constraints via requires clauses.
//
// Usage:
//   import "std/file" as file;
//
//   let handle = file::read_file(fd);
//   let ok = file::write_file(fd, content);

// --- extern declarations: Rust FFI backend ---
extern "Rust" {
    fn file_read(path: i64) -> i64
        requires: path > 0;
        ensures: result >= 0;
    fn file_write(path: i64, content: i64) -> i64
        requires: path > 0 && content >= 0;
        ensures: result >= 0 && result <= 1;
    fn file_exists(path: i64) -> i64
        requires: path > 0;
        ensures: result >= 0 && result <= 1;
    fn file_delete(path: i64) -> i64
        requires: path > 0;
        ensures: result >= 0 && result <= 1;
}

// =============================================================
// Public API: File Read
// =============================================================

// Read file contents as a string handle.
// `path` is an opaque non-negative i64 handle; prefix policy is not
// expressible on this signature (see `safe_read_file` for that).
// Returns 0 on failure, >0 handle on success.
// FFI-backed: contract is enforced by the Rust runtime.
// TRUSTED(FFI): Contract enforced by Rust runtime (serde_json/reqwest/std::fs).
// Z3 verifies contract consistency; body execution delegated to FFI backend.
atom read_file(path: i64)
    effects: [FileRead];
    requires: path > 0;
    ensures: result >= 0;
    body: {
        perform FileRead.read(path);
        file_read(path)
    }

// =============================================================
// Public API: File Write
// =============================================================

// Write content to a file at the given path handle.
// `path` is an opaque positive i64 handle; prefix policy is not
// expressible on this signature (see `safe_write_file` variants).
// Returns 1 on success, 0 on failure.
// FFI-backed: contract is enforced by the Rust runtime.
// TRUSTED(FFI): Contract enforced by Rust runtime (serde_json/reqwest/std::fs).
// Z3 verifies contract consistency; body execution delegated to FFI backend.
atom write_file(path: i64, content: i64)
    effects: [FileWrite];
    requires: path > 0 && content >= 0;
    ensures: result >= 0 && result <= 1;
    body: {
        perform FileWrite.write(path);
        file_write(path, content)
    }

// =============================================================
// Public API: File Utilities
// =============================================================

// Check if a file exists at the given path.
// Returns 1 if exists, 0 if not.
// FFI-backed: contract is enforced by the Rust runtime.
// TRUSTED(FFI): Contract enforced by Rust runtime (serde_json/reqwest/std::fs).
// Z3 verifies contract consistency; body execution delegated to FFI backend.
atom exists(path: i64)
    effects: [FileRead];
    requires: path > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        perform FileRead.read(path);
        file_exists(path)
    }

// Delete a file at the given path.
// Returns 1 on success, 0 on failure.
// FFI-backed: contract is enforced by the Rust runtime.
// TRUSTED(FFI): Contract enforced by Rust runtime (serde_json/reqwest/std::fs).
// Z3 verifies contract consistency; body execution delegated to FFI backend.
atom remove(path: i64)
    effects: [FileWrite];
    requires: path > 0;
    ensures: result >= 0 && result <= 1;
    body: {
        perform FileWrite.write(path);
        file_delete(path)
    }

// =============================================================
// Public API: Safe File Read (Parameterized Effect)
// =============================================================

// Read file contents with compile-time path safety enforcement.
// Uses SafeFileRead parameterized effect which verifies:
//   - Path starts with /tmp/
//   - Path does not contain ".." (directory traversal prevention)
// Returns 1 as a placeholder status code.
atom safe_read_file(path: Str)
    effects: [SafeFileRead(path)];
    requires: starts_with(path, "/tmp/") && not_contains(path, "..");
    ensures: result >= 0;
    body: {
        perform SafeFileRead.read(path);
        1
    }
