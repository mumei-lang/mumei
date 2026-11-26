struct Point {
    x: i64,
    y: i64
}

enum Ev {
    Move(p: Point),
    Idle
}

atom shadowed(p: Point, e: Ev) -> i64
    requires: p.x >= 0;
    ensures: result >= 0;
    body: {
        match e {
            Ev::Move(p) => 1,
            _ => 0
        }
        p.x
    }

atom main() -> i64
    requires: true;
    ensures: true;
    body: {
        0
    }
