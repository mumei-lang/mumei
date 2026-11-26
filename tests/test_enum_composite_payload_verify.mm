// Struct-typed enum payloads on the general verifier's Datatype path.
// A composite payload flattens into one accessor leaf per scalar field:
// `Ev::Move(p)` binds `p` as a fresh struct value over the selector
// leaves, constructors expand bound or literal struct args to leaves,
// and `e.p = <struct>` rewrites the payload leaves while the solver
// proves the tag gate.

struct Point {
    x: i64,
    y: i64
}

enum Ev {
    Move(p: Point),
    Idle
}

// A match arm binds `p` as a composite: `p.x`/`p.y` read the payload
// leaves through the variant's selectors.
atom read_move(e: Ev) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        match e {
            Ev::Move(p) => if p.x >= 0 { p.x } else { 0 },
            _ => 0
        }
    }

// A constructor call with a struct literal expands to leaves: matching
// the value back reads the literal's fields.
atom make_move(v: i64) -> i64
    requires: v >= 0;
    ensures: result == v;
    body: {
        let e = Ev::Move(Point { x: v, y: 0 });
        match e {
            Ev::Move(p) => p.x,
            _ => 0
        }
    }

// A bound struct value expands the same way.
atom move_bound(v: i64) -> i64
    requires: v >= 0;
    ensures: result == v;
    body: {
        let p = Point { x: v, y: v };
        match Ev::Move(p) {
            Ev::Move(q) => q.y,
            _ => 0
        }
    }

// A payload write `e.p = <struct>` rebuilds the value leaf-wise under
// the proven tag: the second match sees the new payload.
atom rewrite(e: Ev, w: i64) -> i64
    requires: w >= 0;
    ensures: result >= 0;
    body: {
        match e {
            Ev::Move(_) => {
                e.p = Point { x: w, y: w };
                match e {
                    Ev::Move(q) => q.x,
                    _ => 0
                }
            },
            _ => 0
        }
    }

// Mixed scalar + composite payloads: leaves flatten in declared order.
enum Pair {
    Both(tag: i64, pt: Point),
    None_
}

atom read_pair(e: Pair) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        match e {
            Pair::Both(t, p) => if t >= 0 && p.y >= 0 { t + p.y } else { 0 },
            _ => 0
        }
    }

atom main() -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        read_move(Ev::Idle) + make_move(1) + move_bound(1)
            + rewrite(Ev::Move(Point { x: 1, y: 1 }), 1)
            + read_pair(Pair::None_)
    }
