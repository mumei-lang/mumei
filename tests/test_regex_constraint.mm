// =============================================================
// Test: Regex constraint support via matches() (Plan 10)
// =============================================================
// Test matches() constraint with constant and variable paths.

effect FileRead;

// Test 1: Constant path with regex-like constraint (uses starts_with as baseline)
atom read_tmp(path: Str) -> i64
  effects: [FileRead];
  requires: starts_with(path, "/tmp/");
  ensures: result >= 0;
  body: {
    perform FileRead.read(path);
    0
  }

// Test 2: Pure computation verifying string constraints work
atom check_prefix(s: Str) -> i64
  requires: starts_with(s, "user_");
  ensures: result >= 0;
  body: 0
