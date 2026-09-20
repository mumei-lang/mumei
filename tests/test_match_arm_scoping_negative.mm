enum Pair { P(i64, i64) }

// The arm writes to the *pattern-bound* `x`, not the outer `x` — the
// outer binding is arm-shadowed, so post-match `x` is still -5 and the
// `result == 7` postcondition must FAIL.
atom arm_write_to_shadowed_name(p: Pair) -> i64
  requires: true;
  ensures: result == 7;
  body: {
    let x = 0 - 5
    let r = match p { Pair::P(x, b) => { x = 7; x } }
    x
  }
