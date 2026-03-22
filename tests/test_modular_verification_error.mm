// =============================================================
// tests/test_modular_verification_error.mm — Cross-atom Error Test
// =============================================================
// Integration test for invalid cross-atom contract composition (P2-A).
// This file is expected to FAIL verification.
// Usage: mumei check tests/test_modular_verification_error.mm
// Expected: InvalidPreState error for bad_pipeline

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
