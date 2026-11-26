enum ReadList {
    Nil,
    Cons(head: i64, tail: ReadList),
}

atom read_recursive_head(l: ReadList) -> i64
    requires: l == ReadList::Cons(4, ReadList::Nil);
    ensures: result == 4;
    body: l.head

atom update_and_read_recursive_head(l: ReadList, new_value: i64) -> bool
    requires: l == ReadList::Cons(4, ReadList::Nil);
    ensures: result == true;
    body: {
        l.head = new_value;
        l.head == new_value
    }
