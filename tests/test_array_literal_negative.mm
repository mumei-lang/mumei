// Negative: literal out-of-bounds read must be rejected — `a` has len 3.
atom lit_oob() -> i64
ensures: result >= 0;
body: {
    let a = [1, 2, 3];
    a[3]
};

// Negative: a while-loop that reassigns `a` must not keep the pre-loop
// literal's store chain — post-loop `a` is a fresh havoc array, so the
// stale claim `a[0] == 1` (from the original [1,2]) must not verify.
atom while_reassign_stale() -> i64
ensures: result == 1;
body: {
    let a = [1, 2];
    let i = 0;
    while i < 1
    invariant: i >= 0 && i <= 1
    decreases: 1 - i
    {
        a = [3, 4, 5];
        i = i + 1
    };
    a[0]
};

// Negative: a mutating callee writes through the shared data pointer —
// `a[0]` after `mutate(a)` must NOT verify as the pre-call element.
atom mutate(arr: [i64]) -> i64
requires: len(arr) >= 1;
ensures: result == arr[0];
body: {
    arr[0] = arr[0] + 10;
    arr[0]
};
atom post_call_stale() -> i64
ensures: result == 1;
body: {
    let a = [1, 2];
    mutate(a);
    a[0]
};

// Regression: rebinding an array-typed name to a scalar must not leave the
// stale `__z3_arr_<name>`/`len_<name>` slots behind — `a[0]` after `a = 5`
// must not read the pre-rebind `[1, 2]` chain.
atom scalar_rebind_stale_read() -> i64
  requires: true;
  ensures: result == 1;
  body: {
    let a = [1, 2]
    a = 5
    a[0]
  };

// Same staleness through a store: `a[0] = 9` after `a = 5` must not write
// into the stale chain.
atom scalar_rebind_stale_store() -> i64
  requires: true;
  ensures: result == 9;
  body: {
    let a = [1, 2]
    a = 5
    a[0] = 9
    a[0]
  };

// Same staleness through a parameter: `arr = 5; arr[0]` must not revive
// the requires-side array (Z3 interns same-named consts — the fallback
// must not re-derive the param symbol).
atom param_rebind_stale(arr: [i64]) -> i64
  requires: len(arr) >= 1 && arr[0] == 4;
  ensures: result == 4;
  body: {
    arr = 5
    arr[0]
  };

// The merged length is per-branch precise: `a[2]` is OOB on the
// `[3, 4]`-side even though the other branch has 3 elements.
atom if_branch_lit_oob(c: bool) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let a = if c { [1, 2] } else { [3, 4] }
    a[2]
  };

// Negative: `match` arm lengths merge per-arm — index 2 is out of bounds
// on the `[1, 2]` arm even though the other arm has 3 elements.
enum LitNegE { LitNegA, LitNegB }

atom match_arm_lit_oob(e: LitNegE) -> i64
ensures: result >= 0;
body: {
    let a = match e {
        LitNegE::LitNegA => [1, 2],
        LitNegE::LitNegB => [3, 4, 5],
    }
    a[2]
};

// Negative: nested `if` inside a `match` arm — index 2 is OOB when the
// inner `if` takes the 2-element branch or the outer match hits the
// 2-element arm.
atom match_arm_nested_if_oob(e: LitNegE, c: bool) -> i64
ensures: result >= 0;
body: {
    let a = match e {
        LitNegE::LitNegA => if c { [1, 2] } else { [9, 9, 9] },
        LitNegE::LitNegB => [3, 4],
    }
    a[2]
};
