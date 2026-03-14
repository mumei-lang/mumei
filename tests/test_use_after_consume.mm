// =============================================================
// Test: Use-after-consume detection
// =============================================================
// Atom A consumes x, then atom B tries to use x after consumption.
// The verification pipeline should detect this via LinearityCtx.check_alive().

atom consumer(consume x: i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: x;

atom use_after_consume(x: i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        let y = consumer(x);
        y
    };
