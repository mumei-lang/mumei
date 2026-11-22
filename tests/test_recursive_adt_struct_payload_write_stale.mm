struct Pt { x: i64, y: i64 }
enum L { Nil, Cons(p: Pt, t: L) }

atom stale_recursive_struct_payload_write(l: L, new_value: i64) -> bool
    requires: match l {
        L::Cons(p, _) => p.x == 2 && new_value == 9,
        _ => false
    };
    ensures: result == true;
    body: {
        let old = l.p.x;
        l.p.x = new_value;
        l.p.x == old
    }
