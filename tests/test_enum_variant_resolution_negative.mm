// `Cons` is owned by both prelude `List<T>` (tag 1) and `IntList` (tag 0)
// and the match target `x + 1` has no declared enum type — the encoding
// cannot be resolved soundly, so this must fail closed, not pick one at
// random.
enum IntList {
    Cons(i64, Self),
    Nil,
}

atom ambiguous_target(x: i64) -> i64
    requires: x >= 0;
    ensures: result >= 0;
    body: {
        match x + 1 {
            Cons(h, t) => 10
            Nil => 20
        }
    }
