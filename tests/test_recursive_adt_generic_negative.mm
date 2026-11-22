enum GenericList<T> {
    Nil,
    Cons(head: T, tail: Self),
}

atom unsupported_generic() -> GenericList<i64>
    requires: true;
    ensures: true;
    body: GenericList::Cons(1, GenericList::Nil)
