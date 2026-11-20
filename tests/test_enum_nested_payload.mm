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
atom nested_payload_assign() -> i64
    requires: true;
    ensures: result == 5;
    body: {
        let e = Ev::Wrap(Box { inner: Point { x: 1, y: 2 } });
        e = Ev::Wrap(Box { inner: Point { x: 5, y: 6 } });
        match e {
            Ev::Wrap(b) => b.inner.x,
            _ => 0
        }
    }
atom nested_payload_aliased() -> i64
    requires: true;
    ensures: result == 8;
    body: {
        let e2 = Ev::Wrap(Box { inner: Point { x: 8, y: 0 } });
        let e = Ev::Idle;
        e = e2;
        match e {
            Ev::Wrap(b) => b.inner.x,
            _ => 0
        }
    }
atom bound_payload_result() -> i64
    requires: true;
    ensures: result == 5;
    body: {
        let e = Ev::Wrap(Box { inner: Point { x: 5, y: 6 } });
        let r = match e {
            Ev::Wrap(b) => b,
            _ => Box { inner: Point { x: 0, y: 0 } }
        };
        r.inner.x
    }
atom bound_payload_returned() -> Box
    requires: true;
    ensures: result.inner.x == 7;
    body: {
        let e = Ev::Wrap(Box { inner: Point { x: 7, y: 3 } });
        match e {
            Ev::Wrap(b) => b,
            _ => Box { inner: Point { x: 0, y: 0 } }
        }
    }
struct InBox {
    inner: Point,
    invariant: self.inner.x >= 0
}
enum GEv {
    GW(b: InBox),
    GIdle
}
atom guarded_payload_returned() -> InBox
    requires: true;
    ensures: result.inner.x == 2;
    body: {
        let e = GEv::GW(InBox { inner: Point { x: 2, y: 9 } });
        match e {
            GEv::GW(b) => b,
            _ => InBox { inner: Point { x: 0, y: 0 } }
        }
    }
atom main() -> i64
    requires: true;
    ensures: true;
    body: {
        0
    }
