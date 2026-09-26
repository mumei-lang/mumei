atom ok(x: i64) -> i64
requires: x == 5;
ensures: result == 8;
body: {
    let acc = 0;
    match x {
        5 => { acc = acc + 3; 0 }
        _ => { acc = acc + 10; 0 }
    };
    acc + 5
}

atom main() -> i64
requires: true;
ensures: result == 8;
body: ok(5);
