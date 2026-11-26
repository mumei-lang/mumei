struct S { x: u64 }

atom f(c: bool, a: u64, b: u64) -> bool
requires: a == 0 - 1 && b == 0 - 1;
ensures: result == true;
body: { (if c { let p = S { x: a }; p.x } else { let q = S { x: b }; q.x }) < 1 };
