// A payload slot typed as a NON-finite inner enum stays on the
// Int-tag encoding — the outer enum is not a finite ADT, so `F::A`
// has no constructible spec-side shape and the atom must reject.
enum L { Nil, Cons(i64, L) }
enum F { A(L), B }

atom wrap(l: L) -> F
    requires: true;
    ensures: true;
    body: { F::A(l) }

atom main() -> i64
    requires: true;
    ensures: true;
    body: { 0 }
