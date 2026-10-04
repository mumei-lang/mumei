// Exact-arity call verifies.

atom add_two(x: i64, y: i64)
    requires: true;
    ensures: result == x + y;
    body: x + y;

atom caller(n: i64)
    requires: n >= 0;
    ensures: result == n + 1;
    body: add_two(n, 1);
