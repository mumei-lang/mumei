atom bad_for_syntax(n: i64) -> i64
    requires: true;
    ensures: true;
    body: {
        for i in 0 { i = i + 1 };
        0
    };
