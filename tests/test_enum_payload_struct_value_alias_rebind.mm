struct P {
    x: i64
}

type PA = P;

struct Q {
    x: i64
}

enum First {
    Move(p: PA),
    Stop
}

enum Second {
    Move(p: PA),
    Stop
}

enum Other {
    Move(p: Q),
    Stop
}

atom alias_rebind_is_same_type(e1: First, e2: Second) -> i64
requires: true;
ensures: true;
body: {
    let q = e1.p;
    let q = e2.p;
    q.x
};

atom different_type_rebind_is_rejected(e1: First, e2: Other) -> i64
requires: true;
ensures: true;
body: {
    let q = e1.p;
    let q = e2.p;
    q.x
};
