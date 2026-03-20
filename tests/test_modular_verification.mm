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

// This should verify successfully: open_file transitions File Closed->Open,
// then write_and_close transitions File Open->Closed
atom full_pipeline(x: i64)
    effects: [File];
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        open_file(x);
        write_and_close(x);
        x
    };
