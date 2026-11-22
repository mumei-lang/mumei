enum UpdateList {
    Nil,
    Cons(head: i64, tail: UpdateList),
}

atom false_after_recursive_payload_update(l: UpdateList, new_value: i64) -> bool
    requires: l == UpdateList::Cons(2, UpdateList::Nil) && new_value == 9;
    ensures: result == true;
    body: {
        let a = l.head;
        l.head = new_value;
        l.head == a
    }
