enum SelfList {
    Nil,
    Cons(value: u32, tail: SelfList),
}

atom invalid_self_recursive_write(l: SelfList, value: i64) -> bool
    requires: l == SelfList::Cons(1, SelfList::Nil) && value == 4294967296;
    ensures: result == true;
    body: {
        l.value = value;
        true
    }
