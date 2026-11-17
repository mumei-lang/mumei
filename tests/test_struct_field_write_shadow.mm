// A composite's env slots are name-keyed (`q_f`, `__struct_q_f`), so a
// `let` shadowing a struct binding with a scalar or a different struct
// would leave stale slots for the branch merge to leak — both forms
// are rejected.

struct Pt {
    x: i64,
    y: i64
}

struct Seg {
    a: i64
}

atom shadow_scalar(q: Pt) -> i64
requires: q.x == 0;
ensures: result == 1;
body: {
    let q = 5;
    q
};

atom shadow_diff_struct(q: Pt) -> i64
requires: q.x == 0;
ensures: result == 1;
body: {
    let q = Seg { a: 9 };
    q.a
};
