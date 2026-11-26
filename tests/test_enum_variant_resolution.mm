// Variant-name collision: IntList declares `Cons = 0`, while the auto-loaded
// prelude `List<T>` declares `Cons = 1`. This no-base recursive enum stays on
// the Int-tag path, and its declared type must pick the right tag.
enum IntList {
    Cons(i64, Self),
}

atom tag_of_cons(l: IntList) -> i64
    requires: l == 0;
    ensures: result == 10;
    body: {
        match l {
            Cons(h, t) => 10
        }
    }
