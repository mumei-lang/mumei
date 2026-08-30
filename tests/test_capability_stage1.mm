// =============================================================
// Capability Model Stage 1: declarations + capability parameters
// =============================================================
// `type X = capability E(...) where C;` declares a capability type: an alias
// over the existing effect `E` carrying a constraint. A capability-typed
// parameter carries the underlying effect in its signature, so the existing
// effect containment rule (parameter effect set ⊆ caller's declared effects)
// applies unchanged.
//
// Usage:
//   mumei verify tests/test_capability_stage1.mm
//
// Expected: all atoms PASS (the caller declares the underlying effect).

effect SafeFileRead(path: Str) where starts_with(path, "/tmp/") && not_contains(path, "..");

type FileCap = capability SafeFileRead(path: Str) where starts_with(path, "/tmp/");

// capability parameter version: `cap: FileCap` contributes SafeFileRead
atom read_via_capability(cap: FileCap, user_id: Str)
    effects: [SafeFileRead(path)]
    requires: not_contains(user_id, "..") && not_contains(user_id, "/") && not_contains(user_id, "\0");
    ensures: result >= 0;
    body: {
        let path = "/tmp/" + user_id + ".log";
        perform cap.read(path);
        1
    }

// equivalent effect-parameter version: same verdict as read_via_capability
atom read_via_effect(user_id: Str)
    effects: [SafeFileRead(path)]
    requires: not_contains(user_id, "..") && not_contains(user_id, "/") && not_contains(user_id, "\0");
    ensures: result >= 0;
    body: {
        let path = "/tmp/" + user_id + ".log";
        perform SafeFileRead.read(path);
        1
    }

// `capability` and `grant` remain ordinary identifiers outside `type X = `
atom capability_identifier_regression(capability: i64, grant: i64)
    requires: capability >= 0 && grant >= 0;
    ensures: result >= 0;
    body: {
        capability + grant
    }
