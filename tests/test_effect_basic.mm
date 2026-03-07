// =============================================================
// Test: Basic effect annotation — should pass verification
// =============================================================
// Atom declares FileWrite effect and uses perform FileWrite.write(x).

effect FileWrite;

atom write_value(x: i64)
effects: [FileWrite];
requires: x >= 0;
ensures: result >= 0;
body: {
    perform FileWrite.write(x);
    x
};
