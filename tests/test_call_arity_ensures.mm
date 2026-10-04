// Wrong-arity call inside an ensures clause reports the arity error.

atom add_two(x: i64, y: i64)
    requires: true;
    ensures: result == x + y;
    body: x + y;

atom caller(n: i64)
    requires: n >= 0;
    ensures: result == add_two(n);
    body: n;
