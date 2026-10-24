// Regression test: unbound names must not silently alias Local(0).
// Before the fix, `task { s }` as a `let` RHS mis-parsed `task` as a
// variable, `lookup_var` fell back to `Local(0)` (the `arr` parameter),
// moved it, and the phantom binding corrupted move analysis.
atom task_arr_read(arr: [i64], i: i64) -> i64
  requires: i >= 0 && i < len(arr);
  ensures: result == result;
  body: {
    let t = task { arr[i] + 1 }
    t.join()
  };

atom unbound_typo(x: i64) -> i64
  requires: true;
  ensures: true;
  body: {
    let y = unknow_fn(x)
    y
  };
