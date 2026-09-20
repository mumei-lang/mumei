// `let a = [e0, e1, …]` — array literals bind a concrete-length Z3 array.
// Previously a `[` prefix fell through to the expression catch-all and
// silently parsed as `Number(0)`, so nothing about the literal was tracked.

atom lit_read() -> i64
ensures: result == 20;
body: {
    let a = [10, 20, 30];
    a[1]
};

atom lit_store() -> i64
ensures: result == 99;
body: {
    let a = [1, 2, 3];
    a[1] = 99;
    a[1]
};

atom lit_len(arr: [i64]) -> i64
requires: len(arr) >= 3 && forall(i, 0, 3, arr[i] >= 0);
ensures: result >= 0;
body: {
    let a = arr;
    a[2]
};

// f64 element literals widen integer elements via sitofp.
atom lit_f64() -> f64
ensures: result == 2.5;
body: {
    let a = [1.0, 2.5, 4.0];
    a[1]
};

// bool element literals stay Bool-sorted.
atom lit_bool() -> bool
ensures: result == true;
body: {
    let a = [true, false, true];
    a[0]
};

// Rebinding to a new literal replaces the tracked array.
atom lit_reassign() -> i64
ensures: result == 7;
body: {
    let a = [1, 2, 3];
    a = [7, 9];
    a[0]
};

// `[e0, …]` as the body tail wires `__z3_arr_result`/`len_result` so
// ensures clauses can quantify over the returned array.
atom lit_tail() -> [i64]
ensures: forall(i, 0, 3, result[i] == i + 1);
body: {
    [1, 2, 3]
};

atom head_lit(arr: [i64]) -> i64
requires: len(arr) >= 1;
ensures: result == arr[0];
body: {
    arr[0]
};

// Array literal as a call argument — the callee's `len(arr) >= 1`
// contract must see the literal's concrete length.
atom lit_call_arg() -> i64
ensures: result == 7;
body: {
    head_lit([7, 8, 9])
};

// A store through the binding must be visible at the `result` tail —
// `__z3_arr_result` wires the post-store chain, not the original const.
atom lit_stored_tail(arr: [i64]) -> [i64]
requires: len(arr) >= 1;
ensures: result[0] == 99;
body: {
    let a = arr;
    a[0] = 99;
    a
};

// `let b = a` on a literal aliases the same tracked array.
atom lit_let_alias() -> i64
ensures: result == 2;
body: {
    let a = [1, 2, 3];
    let b = a;
    b[1]
};

// Symbolic elements from params are allowed.
atom lit_symbolic(x: i64, y: i64) -> i64
requires: x >= 0 && y >= 0;
ensures: result >= 0;
body: {
    let a = [x, y, x + y];
    a[0] + a[1]
};

// A non-mutating callee leaves the caller's tracked chain intact —
// `head(a)` returns a[0] and `a` keeps its literal contents afterwards.
atom lit_head(arr: [i64]) -> i64
requires: len(arr) >= 1;
ensures: result == arr[0];
body: {
    arr[0]
};
atom lit_call_pure() -> i64
ensures: result == 5;
body: {
    let a = [5, 6];
    lit_head(a)
};

// Rebinding to a scalar keeps the scalar value usable (the name itself is
// not poisoned) — only array access on it is rejected.
atom rebind_scalar_tail() -> i64
  requires: true;
  ensures: result == 5;
  body: {
    let a = [1, 2]
    a = 5
    a
  };

// Rebinding back to an array re-wires the slots.
atom rebind_back_to_array() -> i64
  requires: true;
  ensures: result == 7;
  body: {
    let a = [1, 2]
    a = 5
    a = [7, 8]
    a[0]
  };

// Scalar → array rebinding works too.
atom scalar_to_array() -> i64
  requires: true;
  ensures: result == 1;
  body: {
    let x = 5
    x = [1, 2]
    x[0]
  };

// `if`-branch literal lengths merge into `len_<a> = ite(c, 2, 3)` — `a[1]`
// is in bounds on both branches, and `len(a)` provably lies in {2,3}.
atom if_branch_lit_len_read(c: bool) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let a = if c { [1, 2] } else { [3, 4, 5] }
    a[1]
  };

atom if_branch_lit_len_pred(c: bool) -> i64
  requires: true;
  ensures: result >= 2 && result <= 3;
  body: {
    let a = if c { [1, 2] } else { [3, 4, 5] }
    len(a)
  };

atom if_branch_block_tail(c: bool) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let a = if c { let t = [1, 2]; t } else { [3, 4, 5] }
    a[1]
  };

// `let a = match e { … }` merges each arm's array length under its own
// pattern condition — `len_<a>` mirrors the arm chain
// `ite(c_A, 2, ite(c_B, 3, …))` instead of an unconstrained symbol.
enum LitE { LitA, LitB }

atom match_arm_lit_len(e: LitE) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let a = match e {
      LitE::LitA => [1, 2],
      LitE::LitB => [3, 4, 5],
    }
    a[1]
  };

// Arm-local `let`s resolve to their literal rhs — arm slots never reach
// the merged env, so the tail variable's own length would be lost.
atom match_arm_local_let(e: LitE) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let a = match e {
      LitE::LitA => { let t = [1, 2]; t },
      LitE::LitB => [3, 4, 5],
    }
    a[1]
  };

// Var tails contribute the source's tracked length under the arm cond.
atom match_arm_var_len(a1: [i64], e: LitE) -> i64
  requires: len(a1) >= 2;
  ensures: result == result;
  body: {
    let a = match e {
      LitE::LitA => a1,
      LitE::LitB => [3, 4, 5],
    }
    a[1]
  };

// Three+ arms: the len merge folds the whole `ite` spine, not just two.
atom match_arm_three_way(x: i64) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let a = match x {
      0 => [1],
      _ => [1, 2, 3],
    }
    a[0]
  };
