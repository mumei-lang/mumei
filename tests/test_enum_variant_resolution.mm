// Variant-name collision: IntList declares `Cons = 0, Nil = 1`, the
// auto-loaded prelude `List<T>` declares `Nil = 0, Cons = 1`. The match
// target's declared type (`l: IntList`) must pick IntList's tag indices —
// previously `find_enum_by_variant` scanned a HashMap and could return the
// prelude `List`, flipping `Cons` between tag 0 and tag 1 per process.
enum IntList {
    Cons(i64, Self),
    Nil,
}

atom tag_of_nil(l: IntList) -> i64
    requires: l == 1;
    ensures: result == 20;
    body: {
        match l {
            Cons(h, t) => 10
            Nil => 20
        }
    }

atom tag_of_cons(l: IntList) -> i64
    requires: l == 0;
    ensures: result == 10;
    body: {
        match l {
            Cons(h, t) => 10
            Nil => 20
        }
    }
