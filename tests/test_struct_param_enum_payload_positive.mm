struct B { value: u32 }
enum F { None, Some(p: B) }

atom f(b: B) -> F
    requires: true;
    ensures: true;
    body: F::Some(b)
