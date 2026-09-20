// A partial move that IS observed after the merge must still fail closed:
// `r` (a Str payload — a Move type) is moved on the then-path and read after
// the if.
enum Box2 { PS(Str) }

atom partial_move_used(p: Box2, c: i64) -> Str
  requires: true;
  ensures: result == result;
  body: {
    let r = match p { Box2::PS(t) => t }
    let s = if c != 0 { let t = r; t } else { "x" }
    if s == s { r } else { r }
  }
