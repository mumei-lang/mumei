// Negative: a composite payload write `e.p = <struct>` must reject when
// the solver cannot prove the target holds the owning variant — here the
// value is `Idle` (no `p` payload) or an un-narrowed parameter.

struct Point {
    x: i64,
    y: i64
}

enum Ev {
    Move(p: Point),
    Idle
}

atom rewrite_idle(w: i64) -> i64
    requires: true;
    ensures: true;
    body: {
        let e = Ev::Idle;
        e.p = Point { x: w, y: w };
        0
    }
