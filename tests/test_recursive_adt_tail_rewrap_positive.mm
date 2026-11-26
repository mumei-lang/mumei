enum L { Nil, Cons(v: u32, t: L) }

atom f(l: L) -> L
    requires: true;
    ensures: true;
    body: match l {
        L::Nil => L::Nil,
        L::Cons(v, t) => L::Cons(v, t)
    }
