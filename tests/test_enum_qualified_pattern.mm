// Qualified `E::V` match-arm patterns: the qualifier pins the owning enum
// (`parse_pattern` folds `Ident::Ident` into the variant name), so
// `match e { Mine::Yes(v) => .. }` resolves exactly like a bare `Yes(v)`
// arm — and works on both datatype-sorted and Int-tag targets.

enum Mine { Nil, Yes(i64) }
enum Other { Nil, Yes(i64) }

// Datatype-sorted parameter: `Mine::Yes(v)` binds the payload.
atom qual_param(e: Mine) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    match e {
      Mine::Yes(v) => if v >= 0 { v } else { 0 - v }
      Mine::Nil => 9
    }
  }

// Same qualifier as the target's declared enum on an Int-tag (recursive)
// enum: the arm resolves IntList's `Cons`/`Nil` tags.
enum IntList { Cons(i64, IntList), Nil }

atom qual_int_tag(l: IntList) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    match l {
      IntList::Cons(h, t) => if h >= 0 { h } else { 0 - h }
      IntList::Nil => 0
    }
  }

// Qualified arms on a `let`-bound enum value resolve via the inferred
// declared type recorded for the binding.
atom qual_let(n: i64) -> i64
  requires: n >= 0;
  ensures: result >= 0;
  body: {
    let e = Mine::Yes(n)
    match e {
      Mine::Yes(v) => v
      Mine::Nil => 0
    }
  }
