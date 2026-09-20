// The binder scope must be precise: names inside the quantifier body that
// are NOT the binder still fail closed.
atom forall_body_unbound() -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let bad = forall(i, 0, 5, j > 0)
    if bad { 1 } else { 0 }
  };

// The binder must not leak past the quantifier: `i` after the let is
// unbound (it was only in scope inside the body argument).
atom forall_binder_leak() -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let ok = forall(i, 0, 5, i < 10)
    i
  };

// The binder is not in scope inside the bound arguments either:
// `forall(i, i, 5, ...)` uses `i` before it is bound.
atom forall_bound_pos_unbound() -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let ok = forall(i, i, 5, true)
    if ok { 1 } else { 0 }
  };
