enum Ev {
    Move(x: i64),
    Idle
}

atom wide(e: Ev) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        match e {
            Ev::Move(a, b) => 1,
            _ => 0
        }
    }

atom main() -> i64
    requires: true;
    ensures: true;
    body: {
        0
    }
