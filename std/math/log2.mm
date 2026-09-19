// =============================================================
// std/math/log2 — verified integer log2
// =============================================================
// Counts right-shifts until the value reaches 1, which pins the
// result to floor(log2(n)) via the contract (n >> result) == 1.

atom ilog2(n: i64)
    requires: n > 0;
    ensures: result >= 0 && result <= 62 && (n >> result) == 1;
    body: {
        let v = n;
        let r = 0;
        while v > 1
        invariant: v >= 1 && r >= 0 && r <= 62 && (n >> r) == v
        decreases: v
        {
            v = v >> 1;
            r = r + 1;
        };
        r
    };
