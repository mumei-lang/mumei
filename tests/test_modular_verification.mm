// =============================================================
// tests/test_modular_verification.mm — Modular Verification E2E Test
// =============================================================
// Integration test for effect_pre / effect_post contracts (Plan 24).
// Usage: mumei check tests/test_modular_verification.mm

effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition write: Open -> Open;
    transition read: Open -> Open;
    transition close: Open -> Closed;

atom open_file(x: i64)
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Open };
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        perform File.open(x);
        x
    };

atom write_and_close(x: i64)
    effects: [File];
    effect_pre: { File: Open };
    effect_post: { File: Closed };
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        perform File.write(x);
        perform File.close(x);
        x
    };

// This verifies successfully because each atom (open_file, write_and_close)
// is independently verified against its own effect_pre/effect_post contracts.
//
// NOTE: Cross-atom contract composition is NOT yet implemented in the MIR
// temporal analysis. The calls to open_file(x) and write_and_close(x) are
// regular Call operations in MIR, not Perform operations, so no temporal
// state transitions are tracked within full_pipeline's body. This atom
// passes trivially (no temporal violations detected) rather than by
// composing callee contracts at call sites. Future work: when encountering
// a Call to an atom with effect_pre/effect_post, apply the callee's
// post-state as the new current state for the effect.
atom full_pipeline(x: i64)
    effects: [File];
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        open_file(x);
        write_and_close(x);
        x
    };
