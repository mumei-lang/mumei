// Field-write contracts that must NOT verify: the callee's post-state
// ensures propagates to the caller, and a claimed postcondition that
// contradicts the write fails.

struct Pt {
    x: i64,
    y: i64
}

atom callee_bump(q: Pt) -> i64
requires: q.x == 0;
ensures: result == 1 && q.x == 1;
body: {
    q.x = q.x + 1;
    q.x
};

// The callee guarantees `result == 1`; claiming 2 must fail — before
// the call-site post-state fix this atom's requires+ensures collapsed
// vacuously and the failure was masked.
atom caller_bad(p: Pt) -> i64
requires: p.x == 0;
ensures: result == 2;
body: {
    callee_bump(p)
};

atom write_wrong(q: Pt) -> i64
requires: q.x == 0;
ensures: q.x == 6;
body: {
    q.x = 5;
    q.x
};
