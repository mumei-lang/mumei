// Negative: a non-exhaustive `match` on a struct-field scrutinee in a clause
// must fail closed. Previously the exhaustiveness check was skipped when no
// ambient solver was present, so a one-arm `requires: match h.r { V => e }`
// silently degraded to just `e` — the clause evaluated to the last arm's body
// on every uncovered input.
struct H { r: Shape }
enum Shape { Point, Other(i64) }

atom nonexhaustive_requires(h: H) -> i64
requires: match h.r { Shape::Point => true };
ensures: result >= 0;
body: { 0 };

atom nonexhaustive_ensures(h: H) -> i64
requires: true;
ensures: match h.r { Shape::Point => result == 0 };
body: { 0 };

// An exhaustive clause match whose postcondition does not hold must fail.
atom wrong_ensures(h: H) -> i64
requires: match h.r { Shape::Point => true, Shape::Other(v) => v > 0 };
ensures: match h.r { Shape::Point => result == 0, Shape::Other(v) => result == v };
body: { 1 };

// Non-exhaustive match on a non-enum (i64) field fails the same way.
struct N { n: i64 }
atom int_field_nonexhaustive(h: N) -> i64
requires: match h.n { 0 => true };
ensures: result >= 0;
body: { 0 };
