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
