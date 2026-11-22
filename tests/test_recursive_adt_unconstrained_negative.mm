enum L {
    Nil,
    Cons(head: i64, tail: L),
}

atom unsupported_claim(l: L) -> i64
    requires: true;
    ensures: result == 1;
    body: {
        match l {
            L::Nil => 1,
            L::Cons(head, _) => if head == 3 { 1 } else { 0 }
        }
    }
