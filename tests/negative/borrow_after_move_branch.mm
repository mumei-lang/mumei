atom consume(x: i64) -> i64
requires: true;
ensures: result == x;
consume: [x];
body: x;

atom branch_borrow(x: i64, c: bool) -> i64
requires: true;
ensures: true;
body: {
    if c { consume(x) } else { 0 };
    ref x
};
