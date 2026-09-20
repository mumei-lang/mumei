// The binder scope must be precise: names inside the quantifier body that
// are NOT the binder still fail closed.
atom forall_body_unbound() -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let bad = forall(i, 0, 5, j > 0)
    if bad { 1 } else { 0 }
  };
