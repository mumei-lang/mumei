enum L {
    Nil,
    Cons(head: i64, tail: L),
}

enum SelfList {
    Nil,
    Cons(head: i64, tail: Self),
}

enum U32List {
    Nil,
    Cons(value: u32, tail: U32List),
}

atom head_or(l: L, fallback: i64) -> i64
    requires: l == L::Cons(3, L::Nil);
    ensures: result == 3;
    body: {
        match l {
            L::Nil => fallback,
            L::Cons(head, _) => head
        }
    }

atom constructed_head() -> i64
    requires: true;
    ensures: result == 3;
    body: head_or(L::Cons(3, L::Nil), 0)

atom second(l: L) -> i64
    requires: l == L::Cons(1, L::Cons(2, L::Nil));
    ensures: result == 2;
    body: {
        match l {
            L::Nil => 0,
            L::Cons(_, tail) => match tail {
                L::Nil => 0,
                L::Cons(value, _) => value
            }
        }
    }

atom constructed_equality() -> bool
    requires: true;
    ensures: result == true;
    body: L::Cons(1, L::Nil) == L::Cons(1, L::Nil)

atom nil_is_distinct() -> bool
    requires: true;
    ensures: result == true;
    body: L::Nil != L::Cons(0, L::Nil)

atom u32_payload_is_in_range(l: U32List) -> bool
    requires: true;
    ensures: result == true;
    body: {
        match l {
            U32List::Nil => true,
            U32List::Cons(value, _) => value >= 0 && value <= 4294967295
        }
    }

atom list_len(l: L) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        match l {
            L::Nil => 0,
            L::Cons(_, tail) => {
                let n = list_len(tail);
                if n < 9223372036854775807 { n + 1 } else { 0 }
            }
        }
    }

atom self_spelled(s: SelfList) -> i64
    requires: s == SelfList::Cons(7, SelfList::Nil);
    ensures: result == 7;
    body: {
        match s {
            SelfList::Nil => 0,
            SelfList::Cons(head, _) => head
        }
    }
