// An `if`-guard must be usable to discharge a callee's `requires`:
// `callee` needs `x >= 0`, and the call sits inside `if x >= 0`.
// Regression for call-site requires checks ignoring branch conditions.
atom callee(x: i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: x;

atom guarded(x: i64)
    requires: true;
    ensures: result >= 0;
    body: {
        if x >= 0 {
            callee(x)
        } else {
            0
        }
    };
