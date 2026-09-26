trusted atom ok(x: i64) -> i64
requires: x > 2;
ensures: result == 8;
body: {
    let acc = 1;
    if x > 2 {
        let acc = 100;
        acc
    } else {
        0
    };
    acc + 7
}

atom main() -> i64
requires: true;
ensures: result == 8;
body: ok(5);
