// Enum-typed enum payloads on the general verifier's Datatype path.
// A payload slot typed as another finite enum carries the inner enum's
// own datatype sort: a match arm binds `e` as a nested datatype value
// the inner match destructures again, a constructor fills the slot
// with a bound inner value, and `==` descends into the inner leaves
// (`pure-i64-composite/v23`).

enum E { X(i64), Y(i64) }
enum F { A(E), B }

// A nested match reads the inner payload leaf.
atom pick(f: F) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        match f {
            F::A(e) => match e { E::X(v) => if v >= 0 { v } else { 0 }, _ => 0 },
            _ => 0
        }
    }

// A constructor fills the enum-typed slot with a bound inner value;
// matching it back reads the same leaf.
atom wrap(v: i64) -> i64
    requires: v >= 0;
    ensures: result == v;
    body: {
        match F::A(E::X(v)) {
            F::A(e) => match e { E::X(u) => u, _ => 0 },
            _ => 0
        }
    }

// Deep equality descends into the inner tag and payload leaves.
atom eqe(f: F, g: F) -> bool
    requires: true;
    ensures: result == (f == g);
    body: { f == g }

atom main() -> i64
    requires: true;
    ensures: result >= 0;
    body: { wrap(1) + pick(F::A(E::Y(0))) }
