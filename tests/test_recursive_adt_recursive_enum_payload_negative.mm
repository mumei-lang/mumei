enum Inner {
    End,
    More(value: i64, tail: Inner),
}

enum Outer {
    Stop,
    Continue(inner: Inner, tail: Outer),
}

atom recursive_enum_payload_is_not_finite() -> Outer
    requires: true;
    ensures: true;
    body: Outer::Continue(Inner::End, Outer::Stop)
