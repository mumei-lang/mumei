// Too few arguments: must report the arity error, not a requires failure.

atom add_two(x: i64, y: i64)
    requires: true;
    ensures: result == x + y;
    body: x + y;

atom caller(n: i64)
    requires: n >= 0;
    ensures: true;
    body: add_two(n);
