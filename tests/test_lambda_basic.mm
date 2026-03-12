// =============================================================
// Test: Basic Lambda syntax — should pass verification
// =============================================================
// Lambda expressions with various parameter styles and body forms.

atom identity(x: i64)
    requires: x >= 0;
    ensures: result == x;
    body: {
        let f = |a| a;
        call(f, x)
    }

atom double_apply(x: i64)
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        let double = |a: i64| a * 2;
        call(double, x)
    }
