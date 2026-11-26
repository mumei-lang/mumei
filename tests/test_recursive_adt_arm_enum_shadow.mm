enum ShadowList {
    Nil,
    Cons(head: i64, tail: ShadowList),
}

enum OtherEnum {
    A,
    B,
}

atom pattern_binding_does_not_leak(l: ShadowList, t: OtherEnum) -> bool
    requires: true;
    ensures: result == true;
    body: {
        match l {
            ShadowList::Cons(_, t) => true,
            ShadowList::Nil => match t {
                OtherEnum::A => t.tag == 0,
                OtherEnum::B => t.tag == 1
            }
        }
    }
