// =============================================================
// Capability Security Demo: Compile-Time Access Control
// =============================================================
// Demonstrates mumei's capability-based security model using
// parameterized effects with constraints. The compiler (via Z3)
// enforces that all file/network operations satisfy their
// security policies AT COMPILE TIME -- no runtime checks needed.
//
// This demo shows:
//   1. File read restricted to /tmp/ with path traversal prevention
//   2. File write restricted to /tmp/output/ only
//   3. Network access restricted to HTTPS only
//   4. Composition of multiple capability constraints
//   5. Unsafe operations that are REJECTED by the compiler
//
// Usage:
//   mumei check examples/capability_demo.mm
//   mumei verify examples/capability_demo.mm
//
// Expected results:
//   - safe_read_file: PASS
//   - safe_write_output: PASS
//   - safe_https_fetch: PASS
//   - sandboxed_pipeline: PASS
//   - malicious_read_passwd: FAIL (path outside /tmp/)
//   - path_traversal_attack: FAIL (path contains "..")
//   - unsafe_http_fetch: FAIL (URL not constrained to https://)

// =============================================================
// Security Policy Definitions (Capability Boundaries)
// =============================================================

// FileRead: Only paths under /tmp/, no directory traversal
effect SafeFileRead(path: Str) where starts_with(path, "/tmp/") && not_contains(path, "..");

// FileWrite: Only paths under /tmp/output/
effect SafeFileWrite(path: Str) where starts_with(path, "/tmp/output/") && not_contains(path, "..");

// SecureHttp: Only HTTPS URLs allowed
effect SecureHttpGet(url: Str) where starts_with(url, "https://");

// =============================================================
// SAFE Operations (should all PASS verification)
// =============================================================

// Safe file read: user_id is constrained to prevent traversal
atom safe_read_file(user_id: Str)
    effects: [SafeFileRead(path)]
    requires: not_contains(user_id, "..") && not_contains(user_id, "\0") && not_contains(user_id, "/");
    ensures: result >= 0;
    body: {
        let path = "/tmp/" + user_id + ".log";
        perform SafeFileRead.read(path);
        1
    }

// Safe file write: output constrained to /tmp/output/
atom safe_write_output(report_name: Str)
    effects: [SafeFileWrite(path)]
    requires: not_contains(report_name, "..") && not_contains(report_name, "/") && not_contains(report_name, "\0");
    ensures: result >= 0;
    body: {
        let path = "/tmp/output/" + report_name + ".json";
        perform SafeFileWrite.write(path);
        1
    }

// Safe HTTPS fetch: URL prefix guaranteed
atom safe_https_fetch(api_path: Str)
    effects: [SecureHttpGet(url)]
    requires: not_contains(api_path, "..") && not_contains(api_path, " ");
    ensures: result >= 0;
    body: {
        let url = "https://api.example.com/" + api_path;
        perform SecureHttpGet.get(url);
        1
    }

// Composition: read + fetch + write pipeline, all capabilities satisfied
atom sandboxed_pipeline(user_id: Str, api_path: Str, report_name: Str)
    effects: [SafeFileRead(read_path), SecureHttpGet(url), SafeFileWrite(write_path)]
    requires: not_contains(user_id, "..") && not_contains(user_id, "/") && not_contains(user_id, "\0")
           && not_contains(api_path, "..") && not_contains(api_path, " ")
           && not_contains(report_name, "..") && not_contains(report_name, "/") && not_contains(report_name, "\0");
    ensures: result >= 0;
    body: {
        let input_path = "/tmp/" + user_id + ".log";
        perform SafeFileRead.read(input_path);
        let url = "https://api.example.com/" + api_path;
        perform SecureHttpGet.get(url);
        let output_path = "/tmp/output/" + report_name + ".json";
        perform SafeFileWrite.write(output_path);
        1
    }

// =============================================================
// UNSAFE Operations (should all FAIL verification)
// =============================================================

// REJECTED: Attempts to read /etc/passwd (outside /tmp/)
// Z3 proves "/etc/passwd" does not start with "/tmp/"
atom malicious_read_passwd()
    effects: [SafeFileRead(path)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform SafeFileRead.read("/etc/passwd");
        1
    }

// REJECTED: Path traversal via unconstrained user input
// Z3 finds counterexample: user_id = "../../etc/shadow"
atom path_traversal_attack(user_id: Str)
    effects: [SafeFileRead(path)]
    requires: true;
    ensures: result >= 0;
    body: {
        let path = "/tmp/" + user_id;
        perform SafeFileRead.read(path);
        1
    }

// REJECTED: HTTP without HTTPS constraint
// Z3 proves unconstrained url may not start with "https://"
atom unsafe_http_fetch(url: Str)
    effects: [SecureHttpGet(url)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform SecureHttpGet.get(url);
        1
    }
