// Struct field assignment `q.f = v` (`pure-i64-composite/v5`).
// Every atom here must verify: written fields carry their post-write
// value while unwritten fields keep the input-time fact.

struct Pt {
    x: i64,
    y: i64
}

// A written field reads its new value; the binding stays a composite.
atom write_basic(q: Pt) -> i64
requires: q.x == 0 && q.y == 1;
ensures: result == 5 && q.x == 5;
body: {
    q.x = 5;
    q.x
};

// An unwritten sibling field keeps its input-time fact — havocing it
// for an unrelated `q.x` write would make `10 / q.y` unprovable.
atom write_keeps_sibling(q: Pt) -> i64
requires: q.y == 1;
ensures: result == 10 && q.y == 1;
body: {
    q.x = 5;
    10 / q.y
};

// A conditional field write merges per-branch values for that field.
atom write_in_branch(q: Pt) -> i64
requires: q.x >= 0;
ensures: result >= 0;
body: {
    if q.y > 0 {
        q.x = q.x + 1
    } else {
        0
    };
    q.x
};

// `ensures` reads post-write composite fields: the callee's `q.x == 1`
// and `result == 1` clauses both propagate to the caller.
atom callee_bump(q: Pt) -> i64
requires: q.x == 0;
ensures: result == 1 && q.x == 1;
body: {
    q.x = q.x + 1;
    q.x
};

atom caller_use(p: Pt) -> i64
requires: p.x == 0;
ensures: result == 1;
body: {
    callee_bump(p)
};

// Re-binding a composite name to a value of the SAME struct type is a
// full re-init — every `q_f`/`__struct_q_f` slot is overwritten, so no
// stale value survives.
atom rebind_same(q: Pt) -> i64
requires: q.x == 0;
ensures: result == 9;
body: {
    let cur = Pt { x: 5, y: 5 };
    let cur = Pt { x: 9, y: 9 };
    cur.x
};

// Re-binding the struct PARAM itself via `let` counts as a whole-
// aggregate rebind — the input-time requires fact on q.x must be
// dropped so the post-reinit ensures is consistent, not a spec
// contradiction.
atom rebind_param(q: Pt) -> i64
requires: q.x == 0 && q.y == 0;
ensures: result == 9 && q.x == 9;
body: {
    let q = Pt { x: 9, y: 9 };
    q.x
};

// A `let` re-init whose right side is a branch expression still
// resolves to the same struct type — `if`/`match` tails keep their
// declared type.
atom rebind_branch_rhs(b: i64, q: Pt) -> i64
requires: b == 0 || b == 1;
ensures: result >= 1 && q.x >= 1;
body: {
    let q = if b == 0 { Pt { x: 1, y: 0 } } else { Pt { x: 2, y: 0 } };
    q.x
};

// A `match` right side works too: each arm's StructInit registers its
// fresh handle's `__struct_<h>_f` slots in the arm env, and the merge
// must carry them so `project_struct_fields` can fold the field-wise
// ite — otherwise the rebound q.x is unconstrained.
atom rebind_match_rhs(b: i64, q: Pt) -> i64
requires: b == 0 || b == 1;
ensures: result >= 1 && q.x >= 1;
body: {
    let q = match b { 0 => Pt { x: 3, y: 0 }, _ => Pt { x: 4, y: 0 } };
    q.x
};

// A recursive callee that writes a composite field still propagates
// its ensures to a caller whose requires constrains the input
// field — the recursion antecedent evaluates on the input-time
// slots, not the post-write fresh constants. (Composite-field
// post-state inside the recursive call in its own body is tracked
// through rec_fn's result only — a deeper limitation.)
atom rec_bump(n: i64, q: Pt) -> i64
requires: n >= 0 && q.x == 0;
ensures: result >= 1;
body: {
    if n == 0 {
        q.x = q.x + 1;
        q.x
    } else {
        rec_bump(n - 1, q)
    }
};

atom caller_rec(p: Pt) -> i64
requires: p.x == 0;
ensures: result >= 1;
body: {
    rec_bump(1, p)
};

// Field writes inside a loop havoc the composite base on entry —
// post-loop field facts are lost, but unrelated locals still verify.
atom write_in_loop(q: Pt) -> i64
requires: q.x >= 0;
ensures: result == 5;
body: {
    let i = 0;
    while i < 3
    invariant: i >= 0 && i <= 3
    decreases: 3 - i
    {
        q.x = q.x + 1;
        i = i + 1;
    };
    let j = 5;
    j
};
