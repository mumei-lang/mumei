enum Mine { Cons(i64), Nil }

// A lowercase `qual::` prefix in a pattern is a path, not a binding —
// `mine::Cons(v)` must bind `v` to the payload.
atom lower_qual_binds_payload() -> i64
  requires: true;
  ensures: result == 42;
  body: {
    match Mine::Cons(41) { mine::Cons(v) => v + 1, Mine::Nil => 0 }
  }

// Unknown module-path qualifier still resolves the variant by leaf name
// (module-path support), matching the bare-variant behavior.
atom lower_qual_leaf_resolution(p: Mine) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    match p { some_mod::Cons(v) => if v > 0 { v } else { 0 }, Mine::Nil => 0 }
  }
