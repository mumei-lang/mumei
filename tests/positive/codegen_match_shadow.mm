enum Mine { Cons(i64), Nil }

atom ok(m: Mine) -> i64
requires: true;
ensures: result == 8;
body: {
    let x = 3;
    match m {
        Cons(x) => x
        Nil => 0
    };
    x + 5
}

atom main() -> i64
requires: true;
ensures: result == 8;
body: ok(Mine::Cons(100));
