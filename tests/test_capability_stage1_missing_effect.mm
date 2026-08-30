// =============================================================
// Capability Model Stage 1: negative case
// =============================================================
// The atom takes a capability-typed parameter but does not declare the
// underlying effect, so the existing effect containment rule (parameter
// effect set ⊆ caller's declared effects) rejects it.
//
// Usage:
//   mumei verify tests/test_capability_stage1_missing_effect.mm
//
// Expected: FAIL (effect polymorphism violation: SafeFileRead is missing).

effect SafeFileRead(path: Str) where starts_with(path, "/tmp/");

type FileCap = capability SafeFileRead(path: Str) where starts_with(path, "/tmp/");

atom read_without_declaring_effect(cap: FileCap, user_id: Str)
    requires: not_contains(user_id, "..") && not_contains(user_id, "/");
    ensures: result >= 0;
    body: {
        let path = "/tmp/" + user_id + ".log";
        perform cap.read(path);
        1
    }
