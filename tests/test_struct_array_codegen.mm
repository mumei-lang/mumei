struct P {
    x: i64,
    y: i64
}

atom main() -> i64
requires: true;
ensures: true;
body: {
    let ps = [P { x: 1, y: 2 }, P { x: 3, y: 4 }];
    ps[1] = P { x: 7, y: 8 };
    let p = ps[1];
    ps[0].x + ps[1].y + p.x
};
