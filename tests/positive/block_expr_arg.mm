trusted atom h(a: i64, b: i64) -> i64
requires: true;
ensures: result == b;
body: b;

atom ok(x: i64) -> i64
requires: x == 0;
ensures: result == 2;
body: {
    let y = {
        let t = x + 1;
        t * 2
    };
    h({ x + 1 }, y)
};
