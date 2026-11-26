enum F { None, Some(v: u32) }

atom f(n: i64) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        let l = F::Some(n);
        match l { F::None => 0, F::Some(v) => n }
    }
