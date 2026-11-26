// Struct-typed enum payloads under --bitvec-i64: the constructor args
// and param leaves are BV(64), so the accessor sorts must match.

struct Point {
    x: i64,
    y: i64
}

enum Ev {
    Move(p: Point),
    Idle
}

atom bump() -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        let e = Ev::Move(Point { x: 1, y: 2 });
        match e {
            Ev::Move(p) => p.x + p.y,
            _ => 0
        }
    }

atom main() -> i64
    requires: true;
    ensures: true;
    body: {
        0
    }
