// Negative fixture: `if` without `else` must be a clean syntax error,
// not a panic. if-expressions always require both branches.
atom missing_else()
    requires: true;
    ensures: result >= 0;
    body: {
        let x = if 1 > 0 { 5 };
        x
    };
