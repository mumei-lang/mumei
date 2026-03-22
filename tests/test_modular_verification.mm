// =============================================================
// tests/test_modular_verification.mm — Modular Verification E2E Test
// =============================================================
// Integration test for effect_pre / effect_post contracts (Plan 24).
// Includes cross-atom contract composition (P2-A).
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

// Cross-atom contract composition (P2-A):
// The calls to open_file(x) and write_and_close(x) are now tracked
// as temporal state transitions via their effect_pre/effect_post contracts.
// open_file requires File:Closed (initial state) and produces File:Open.
// write_and_close requires File:Open and produces File:Closed.
// This sequence is valid: Closed -> Open -> Closed.
atom full_pipeline(x: i64)
    effects: [File];
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        open_file(x);
        write_and_close(x);
        x
    };

// Invalid cross-atom composition: write_and_close is called first,
// but it requires File:Open while the initial state is File:Closed.
// This should produce an InvalidPreState verification error.
atom bad_pipeline(x: i64)
    effects: [File];
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        write_and_close(x);
        open_file(x);
        x
    };
