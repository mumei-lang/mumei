// `!` on a non-bool operand must fail closed — the desugared condition is
// an if-condition, which requires Bool.
atom bang_int_operand(n: i64) -> i64
  requires: n >= 0;
  ensures: result >= 0;
  body: {
    if !n { 0 } else { 1 }
  }
