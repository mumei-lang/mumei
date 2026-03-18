// =============================================================
// Test: Dynamic path construction verification (Plan 10)
// =============================================================
// Verify that Z3 can prove dynamically constructed strings
// (via concatenation) satisfy effect where-clause constraints.

effect FileRead;
effect FileWrite;

// Test 1: Simple variable path with requires constraint
atom read_safe_path(path: Str) -> i64
  effects: [FileRead];
  requires: starts_with(path, "/tmp/");
  ensures: result >= 0;
  body: {
    perform FileRead.read(path);
    0
  }

// Test 2: Pure computation with string parameters
atom build_path(prefix: Str, suffix: Str) -> i64
  requires: starts_with(prefix, "/tmp/");
  ensures: result >= 0;
  body: 0
