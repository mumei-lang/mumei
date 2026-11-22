struct B { value: u32 }
enum L { Nil, Cons(p: B, t: L) }

atom f(n: i64) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        let l = L::Cons(B { value: n }, L::Nil);
        match l { L::Nil => 0, L::Cons(p, _) => n }
    }
