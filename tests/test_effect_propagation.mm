// =============================================================
// Test: Effect propagation — should pass verification
// =============================================================
// Caller's effects must be a superset of callee's effects.
// write_and_log has [Log, FileWrite], logger has [Log].
// write_and_log's effects include Log, so calling logger is safe.

effect FileWrite;
effect Log;

atom logger(msg: i64)
effects: [Log];
requires: msg >= 0;
ensures: result >= 0;
body: {
    perform Log.info(msg);
    msg
};

atom write_and_log(x: i64)
effects: [Log, FileWrite];
requires: x >= 0;
ensures: result >= 0;
body: {
    perform FileWrite.write(x);
    logger(x)
};
