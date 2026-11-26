struct Pt {
    x: i64,
    y: i64,
}

enum L {
    Nil,
    Cons(p: Pt, t: L),
}

atom false_struct_payload_leaf(l: L) -> bool
    requires: true;
    ensures: result == true;
    body: {
        match l {
            L::Nil => true,
            L::Cons(p, _) => p.x == 0
        }
    }
