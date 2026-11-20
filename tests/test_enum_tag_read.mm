enum Ev {
    Move(x: i64),
    Idle
}
enum IntList {
    Nil,
    Cons(h: i64, t: IntList)
}
atom tag_range(e: Ev) -> i64
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        e.tag
    }
atom tag_narrows(e: Ev) -> i64
    requires: e.tag == 1;
    ensures: result == 1;
    body: {
        match e {
            Ev::Move(_) => 0,
            _ => 1
        }
    }
atom ctor_tag() -> i64
    requires: true;
    ensures: result == 0;
    body: {
        let e = Ev::Move(7);
        e.tag
    }
atom recursive_tag(l: IntList) -> i64
    requires: true;
    ensures: result >= 0 && result <= 1;
    body: {
        l.tag
    }
atom main() -> i64
    requires: true;
    ensures: true;
    body: {
        0
    }
