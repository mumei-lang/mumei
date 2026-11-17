enum Pair { P(i64, i64) }

// A pattern binding that shadows an outer `let` must stay arm-local:
// post-match `x` is still the outer 99.
atom pattern_shadow_stays_local(p: Pair) -> i64
  requires: true;
  ensures: result == 100;
  body: {
    let x = 99
    let r = match p { Pair::P(x, b) => x + b }
    if x > 0 { 100 } else { 0 - 100 }
  }

// A `let` inside an arm body that shadows an outer name is arm-local too.
atom let_shadow_stays_local(p: Pair) -> i64
  requires: true;
  ensures: result == 99;
  body: {
    let x = 99
    let r = match p { Pair::P(q, b) => { let x = q + b; x } }
    x
  }

// Real assignments to outer variables still fold back under the arm
// condition — arm-local scoping only covers bound names, not writes.
atom outer_assign_still_folds(p: Pair) -> i64
  requires: true;
  ensures: result == 7;
  body: {
    let x = 99
    let r = match p { Pair::P(q, b) => { x = 7; q + b } }
    x
  }

// First-match precedence for arm writes: an earlier arm's assignment must
// override a later arm's, so post-match `y` is 5 when x == 1 (not the
// trailing `_` arm's 7).
atom earlier_arm_write_wins(x: i64) -> i64
  requires: x >= 0 && x <= 2;
  ensures: x != 1 || result == 5;
  body: {
    let y = 0
    let r = match x { 1 => { y = 5; y }, _ => { y = 7; y } }
    y
  }
