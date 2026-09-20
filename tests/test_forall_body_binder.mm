// Regression: quantifier-style calls used in a body `let` must scope the
// binder (the first argument) so it is not reported as unbound.
// Previously `forall(i, 0, n, i <= n)` recorded `i` as an unbound name and
// Phase 1h rejected the atom.
atom forall_body_ok(n: i64) -> i64
  requires: n >= 1;
  ensures: result == 1;
  body: {
    let ok = forall(i, 0, n, i <= n)
    if ok { 1 } else { 0 }
  };

atom exists_body_ok() -> i64
  requires: true;
  ensures: result == 1;
  body: {
    let e = exists(i, 0, 5, i == 3)
    if e { 1 } else { 0 }
  };

atom forall_body_false(n: i64) -> i64
  requires: n >= 1;
  ensures: result == 0;
  body: {
    let ok = forall(i, 0, n, i < 0)
    if ok { 1 } else { 0 }
  };
