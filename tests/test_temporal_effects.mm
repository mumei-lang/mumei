// =============================================================
// tests/test_temporal_effects.mm — Temporal Effect Verification
// =============================================================
// Integration test for Task 3: Stateful Effects
//
// This file defines a stateful effect (File) with states and transitions,
// then uses it in atoms to verify temporal ordering at compile time.

// Define a stateful File effect with Open/Closed states
effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition write: Open -> Open;
    transition read: Open -> Open;
    transition close: Open -> Closed;

// Valid usage: open -> write -> close (should verify successfully)
atom valid_file_usage(x: i64)
    requires: x >= 0;
    ensures: result >= 0;
    effects: [File];
    body: {
        perform File.open(x);
        perform File.write(x);
        perform File.close(x);
        x
    };

// NOTE: To test that Phase 1i rejects invalid temporal ordering, uncomment
// the atom below. It performs write before open, which should produce an
// InvalidPreState error at compile time:
//
// atom invalid_file_usage(x: i64)
//     requires: x >= 0;
//     ensures: result >= 0;
//     effects: [File];
//     body: {
//         perform File.write(x);
//         perform File.open(x);
//         x
//     };
