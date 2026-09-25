atom bad_for_invariant(n: i64) -> i64
    requires: n >= 1;
    ensures: result == n;
    body: {
        let acc = 0;
        for i in 0..n invariant: acc == 1 { acc = acc + 1 };
        acc
    };
