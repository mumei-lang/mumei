// A partial move that IS observed after the merge must still fail closed:
// `r` is moved on the then-path and read after the if.
enum Pair { P(i64, i64) }

atom partial_move_used(p: Pair, c: i64) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let r = match p { Pair::P(a, b) => a }
    let s = if c != 0 { let t = r; t } else { 0 }
    r + s
  }
