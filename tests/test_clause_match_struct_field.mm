// `match` on a struct-field scrutinee inside requires:/ensures: clauses.
// The scrutinee's declared enum type resolves through the field chain
// (param h: H -> struct H -> field r -> enum Shape).
struct H { r: Shape }
enum Shape { Point, Other(i64) }

// Clause match on h.r in both requires and ensures, binding Other's payload.
atom field_match(h: H) -> i64
requires: match h.r { Shape::Point => true, Shape::Other(v) => v > 0 };
ensures: match h.r { Shape::Point => result == 0, Shape::Other(v) => result == 1 };
body: { match h.r { Shape::Point => 0, Shape::Other(v) => 1 } };

// Bare variant names resolve through the field's declared type as well.
atom bare_variant_match(h: H) -> i64
requires: match h.r { Point => true, Other(v) => v > 0 };
ensures: match h.r { Point => result == 0, Other(v) => result == 1 };
body: { match h.r { Point => 0, Other(v) => 1 } };

// Nested field chain: o.a.r walks Outer -> Inner -> Shape.
struct Inner { r: Shape }
struct Outer { a: Inner, n: i64 }
atom nested_field_match(o: Outer) -> i64
requires: match o.a.r { Shape::Point => true, Shape::Other(v) => v >= 0 };
ensures: match o.a.r { Shape::Point => result >= 0, Shape::Other(v) => result >= v };
body: { match o.a.r { Shape::Point => 0, Shape::Other(v) => v } };

// `let s = h.r` records s: Shape, so `match s` resolves like `match h.r`.
atom let_bound_field(h: H) -> i64
requires: match h.r { Shape::Point => true, Shape::Other(v) => v > 0 };
ensures: result >= 0 && result <= 1;
body: { let s = h.r match s { Shape::Point => 0, Shape::Other(v) => 1 } };

// A match on a non-enum (i64) field behaves exactly like a body-level
// i64 match: literal patterns plus a wildcard.
struct N { n: i64 }
atom int_field_match(h: N) -> i64
requires: match h.n { 0 => true, _ => h.n > 0 };
ensures: match h.n { 0 => result == 0, _ => result == 1 };
body: { if h.n == 0 { 0 } else { 1 } };
