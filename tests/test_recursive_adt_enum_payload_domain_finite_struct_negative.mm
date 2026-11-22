struct B { value: u32 }
enum F { None, Some(p: B) }

atom f(n: i64) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        let l = F::Some(B { value: n });
        match l { F::None => 0, F::Some(p) => n }
    }
