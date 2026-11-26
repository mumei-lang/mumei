// A field-projection const seeded from an *actual argument* is named after
// the caller's binding (`s_v`), so the caller's parameter type — not a
// same-named callee parameter — must own the field-path resolution.
enum E1 {
    A,
    N(next: E1)
}
enum E2 {
    C,
    D
}
struct HolderA {
    v: E1
}
struct HolderB {
    v: E2
}
atom callee(h: HolderA, s: HolderB) -> i64
    requires: match h.v { E1::A => true, _ => false };
    ensures: result == 1;
    body: {
        1
    }
atom caller(s: HolderA, w: HolderB) -> i64
    requires: s.v.tag == 0;
    ensures: result == 1;
    body: {
        callee(s, w)
    }
