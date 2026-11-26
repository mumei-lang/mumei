enum W {
    A(u64),
    B
}

atom double(e: W) -> i64
requires: true;
ensures: result == 1;
body: {
    match e {
        W::A(x) => if x + x >= x { 1 } else { 0 },
        _ => 1
    }
}
