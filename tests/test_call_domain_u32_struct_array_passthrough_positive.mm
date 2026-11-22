struct B { values: [u32] }

atom g(b: B) -> bool
    requires: true;
    ensures: true;
    body: true

atom f(b: B) -> bool
    requires: true;
    ensures: true;
    body: g(b)
