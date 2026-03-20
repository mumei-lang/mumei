// =============================================================
// tests/test_regex_policy.mm — Regex Path Policy E2E Test
// =============================================================
// Integration test for regex-based path constraints (Plan 23).
// Usage: mumei check tests/test_regex_policy.mm

effect RegexSafeFileRead(path: Str) where matches(path, "^/tmp/[a-z]+/.*");

// Test 1: Valid literal path matching regex
atom test_valid_path()
    effects: [RegexSafeFileRead(path)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform RegexSafeFileRead.read("/tmp/data/file.txt");
        1
    };

// Test 2: Another valid literal path
atom test_valid_nested_path()
    effects: [RegexSafeFileRead(path)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform RegexSafeFileRead.read("/tmp/logs/access.log");
        1
    };
