enum L {
    Nil,
    Cons(h: i64, t: L)
}

atom head_or(l: L, d: i64) -> i64
requires: true;
ensures: true;
body: {
    match l {
        L::Cons(h, t) => h,
        L::Nil => d
    }
};

atom mk() -> i64
requires: true;
ensures: result == 3;
body: {
    let l = L::Cons(3, L::Nil);
    head_or(l, 0)
};
