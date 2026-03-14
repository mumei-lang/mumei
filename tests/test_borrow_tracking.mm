// =============================================================
// Test: Borrow tracking via ref parameters
// =============================================================
// Atom A borrows x via ref, verifying that borrow tracking is wired
// into the verification pipeline via LinearityCtx.borrow()/release_borrow().

atom reader(ref x: i64)
    requires: x >= 0;
    ensures: result == x;
    body: x;

atom borrow_and_use(x: i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        let y = reader(x);
        y
    };
