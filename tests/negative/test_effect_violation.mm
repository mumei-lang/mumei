// =============================================================
// Negative Test: Pure atom performing an effect — should FAIL
// =============================================================
// A pure atom (no effects: annotation) cannot use perform.

effect FileWrite;

atom pure_but_writes(x: i64)
requires: x >= 0;
ensures: result >= 0;
body: {
    perform FileWrite.write(x);
    x
};
