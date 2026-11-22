struct P {
    x: i64,
    y: i64
}

atom main() -> i64
requires: true;
ensures: result == 63;
body: {
    let ps = [P { x: 0, y: 0 }];
    let qs = [P { x: 0, y: 0 }];
    let xs = [0];
    let i = 0;
    ps[{ i = i + 1; 0 }].x = { i = i + 10; i };
    qs[{ i = i + 1; 0 }] = {
        i = i + 10;
        P { x: i, y: 0 }
    };
    xs[{ i = i + 1; 0 }] = { i = i + 10; i };
    ps[0].x + qs[0].x + xs[0]
};
