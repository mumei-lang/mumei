// =============================================================
// Test: Capability Security Evaluation
// =============================================================
// Stress-tests the parameterized effect system for capability-based security.

// Test 1: Simple path constraint (should pass)
// Effect with parameterized constraint on file path.
effect FileRead(path: Str) where starts_with(path, "/tmp/");

atom read_tmp(filename: i64)
    effects: [FileRead];
    requires: filename >= 0;
    ensures: result >= 0;
    body: {
        perform FileRead.read(filename);
        0
    };

// Test 2: Path delegation through call chain
// Caller delegates FileRead capability to callee with narrower constraint.
atom delegated_read(filename: i64)
    effects: [FileRead];
    requires: filename >= 0;
    ensures: result >= 0;
    body: read_tmp(filename);

// Test 3: Pure computation (no effects needed)
// Demonstrates that atoms without effects remain pure.
atom pure_transform(x: i64)
    requires: x >= 0;
    ensures: result == x + 1;
    body: x + 1;

// Test 4: Multiple effects with containment
effect Log;

atom read_and_log(filename: i64)
    effects: [FileRead, Log];
    requires: filename >= 0;
    ensures: result >= 0;
    body: {
        perform FileRead.read(filename);
        perform Log.info(filename);
        0
    };
