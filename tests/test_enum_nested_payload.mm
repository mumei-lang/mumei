struct Point {
    x: i64,
    y: i64
}
struct Box {
    inner: Point
}
enum Ev {
    Wrap(b: Box),
    Idle
}
atom nested_ctor_read() -> i64
    requires: true;
    ensures: result == 3;
    body: {
        let e = Ev::Wrap(Box { inner: Point { x: 3, y: 9 } });
        match e {
            Ev::Wrap(b) => b.inner.x,
            _ => 0
        }
    }
atom nested_payload_param(e: Ev) -> i64
    requires: true;
    ensures: result == 0;
    body: {
        match e {
            Ev::Wrap(b) => b.inner.y - b.inner.y,
            _ => 0
        }
    }
atom nested_two_leaf_read() -> i64
    requires: true;
    ensures: result == 7;
    body: {
        let e = Ev::Wrap(Box { inner: Point { x: 1, y: 2 } });
        match e {
            Ev::Wrap(b) => b.inner.x + b.inner.y + 4,
            _ => 0
        }
    }
atom main() -> i64
    requires: true;
    ensures: true;
    body: {
        0
    }
