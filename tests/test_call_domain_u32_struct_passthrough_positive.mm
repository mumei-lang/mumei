struct B { value: u32 }

atom g(b: B) -> i64
    requires: true;
    ensures: result == b.value;
    body: b.value

atom f(b: B) -> i64
    requires: true;
    ensures: result == b.value;
    body: g(b)
