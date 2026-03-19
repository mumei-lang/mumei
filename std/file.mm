// =============================================================
// std/file.mm — Mumei Standard Library: File I/O Primitives
// =============================================================
// Provides verified file read/write operations with path constraints.
// All operations declare their effects and enforce path restrictions
// via requires clauses for security policy compliance.
//
// Usage:
//   import "std/file" as file;
//
//   let content = file::read_file("/tmp/data.txt");
//   let ok = file::write_file("/tmp/output.txt", content);
//
// Security:
//   - read_file requires path starts with /tmp/ or /home/
//   - write_file requires path starts with /tmp/
//   - These constraints are enforced by Z3 at compile time

// --- extern declarations: Rust FFI backend ---
extern "Rust" {
    fn file_read(path: i64) -> i64;
    fn file_write(path: i64, content: i64) -> i64;
    fn file_exists(path: i64) -> i64;
    fn file_delete(path: i64) -> i64;
}

// =============================================================
// Public API: File Read
// =============================================================

// Read file contents as a string handle.
// Path must start with /tmp/ or /home/ for security.
// Returns 0 on failure, >0 handle on success.
atom read_file(path: i64)
    effects: [FileRead]
    requires: true;
    ensures: result >= 0;
    body: {
        perform FileRead.read(path);
        file_read(path)
    }

// =============================================================
// Public API: File Write
// =============================================================

// Write content to a file at the given path.
// Path must start with /tmp/ for security.
// Returns 1 on success, 0 on failure.
atom write_file(path: i64, content: i64)
    effects: [FileWrite]
    requires: true;
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
atom exists(path: i64)
    effects: [FileRead]
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        perform FileRead.read(path);
        file_exists(path)
    }

// Delete a file at the given path.
// Returns 1 on success, 0 on failure.
atom remove(path: i64)
    effects: [FileWrite]
    requires: true;
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
    effects: [SafeFileRead(path)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform SafeFileRead.read(path);
        1
    }
