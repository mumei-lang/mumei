enum NestedSelfWriteList {
    Nil,
    Cons(head: i64, tail: NestedSelfWriteList),
}

atom replace_nested_self_field(l: NestedSelfWriteList, new_head: i64) -> NestedSelfWriteList
    requires: l == NestedSelfWriteList::Cons(1, NestedSelfWriteList::Nil);
    ensures: true;
    body: {
        l.tail.head = new_head;
        l
    }
