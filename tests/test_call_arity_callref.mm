// Wrong-arity call through call(atom_ref(f), ...) reports the arity error
// on the CallRef atom-binding path.

atom add_two(x: i64, y: i64)
    requires: true;
    ensures: result == x + y;
    body: x + y;

atom caller(n: i64)
    requires: n >= 0;
    ensures: true;
    body: call(atom_ref(add_two), n);
