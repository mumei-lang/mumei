// MIR match lowering must bind pattern variables to real locals (previously
// they fell back to Local(0), making `let q = p; match q { Mk(a,s,f) => .. }`
// a spurious "use of moved value" on the parameter), and conditional moves
// that are unused after the merge are not ownership conflicts.

enum Triple { Mk(i64, i64, i64) }
enum Pair { P(i64, i64) }
enum Outer { Wrap(Pair) }

atom bind3_param(p: Triple) -> i64
  requires: true;
  ensures: result == result;
  body: {
    let q = p
    match q {
      Triple::Mk(a, s, f) => a + s + f
    }
  }

atom bind3_local() -> i64
  requires: true;
  ensures: result == 6;
  body: {
    let q = Triple::Mk(1, 2, 3)
    match q {
      Triple::Mk(a, s, f) => a + s + f
    }
  }

atom if_tail_move(p: Pair, c: i64) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let r = match p { Pair::P(a, b) => a * a + b * b }
    if c != 0 { r } else { 0 }
  }

atom nested_field_binding(o: Outer) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    match o {
      Outer::Wrap(Pair::P(a, b)) => a * a + b * b
    }
  }

atom var_arm_consumes(p: Pair) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    let r = match p { whole => 1 }
    r + 1
  }
