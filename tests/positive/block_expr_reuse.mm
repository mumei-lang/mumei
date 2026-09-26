atom f(x: i64) -> i64
requires: true;
ensures: result == x * 2 + 2;
body: {
    let y = { let t = x + 1; t };
    let z = y;
    y + z
};
