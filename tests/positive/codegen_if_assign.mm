atom ok(x: i64) -> i64
requires: x > 2;
ensures: result == x + 3;
body: {
    let acc = x;
    if x > 2 {
        acc = acc + 1;
    } else {
        acc = acc + 10;
    }
    let z = acc + 2;
    z
}

atom main() -> i64
requires: true;
ensures: result == 8;
body: ok(5);
