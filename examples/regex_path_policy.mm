// =============================================================
// Regex Path Policy: Compile-Time Path Validation with Regex
// =============================================================
// Demonstrates regex-based path constraints using matches().

effect RegexSafeFileRead(path: Str) where matches(path, "^/tmp/[a-z]+/.*");

// Valid: literal path matching regex
atom read_safe_file()
    effects: [RegexSafeFileRead(path)]
    requires: true;
    ensures: result >= 0;
    body: {
        perform RegexSafeFileRead.read("/tmp/data/log.txt");
        1
    };
