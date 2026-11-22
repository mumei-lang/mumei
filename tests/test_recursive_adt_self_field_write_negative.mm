enum SelfWriteList {
    Nil,
    Cons(head: i64, tail: SelfWriteList),
}

atom replace_self_field(l: SelfWriteList, new_tail: SelfWriteList) -> SelfWriteList
    requires: l == SelfWriteList::Cons(1, SelfWriteList::Nil);
    ensures: true;
    body: {
        l.tail = new_tail;
        l
    }
