enum L { Nil, Cons(v: u32, t: L) }

atom f(n: i64) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        let l = L::Cons(n, L::Nil);
        match l { L::Nil => 0, L::Cons(v, _) => n }
    }
