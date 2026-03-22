// =============================================================
// tests/test_cross_atom_chain.mm — Chained Cross-atom Composition
// =============================================================
// E2E test for chained cross-atom contract composition (P2-A).
// Verifies that contracts propagate correctly through A -> B -> C call chains.
// Usage: mumei check tests/test_cross_atom_chain.mm

effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition read: Open -> Open;
    transition write: Open -> Open;
    transition close: Open -> Closed;

// A: Opens a file (Closed -> Open)
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

// B: Reads from an open file (Open -> Open)
atom read_file(x: i64)
    effects: [File];
    effect_pre: { File: Open };
    effect_post: { File: Open };
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        perform File.read(x);
        x
    };

// C: Writes and closes a file (Open -> Closed)
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

// Chained cross-atom composition: A -> B -> C
// File: Closed -> Open (open_file) -> Open (read_file) -> Closed (write_and_close)
// Each callee's effect_pre is checked against the current state,
// and effect_post is propagated to the next call site.
atom chained_pipeline(x: i64)
    effects: [File];
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        open_file(x);
        read_file(x);
        write_and_close(x);
        x
    };

// Two-level chain: open then immediately close via write_and_close
atom simple_chain(x: i64)
    effects: [File];
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        open_file(x);
        write_and_close(x);
        x
    };
