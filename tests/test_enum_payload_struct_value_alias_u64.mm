struct Inner {
    x: u64
}

type IA = Inner;

struct P {
    x: u64,
    inner: IA,
    tail: i64
}

type PA = P;

enum Ev {
    Move(p: PA),
    Stop
}

atom alias_positive_top(e: Ev, x: u64) -> bool
requires: match e {
    Ev::Move(p) => p.x == x && p.inner.x == x,
    _ => false
} && x == 0 - 1;
ensures: result == false;
body: {
    let q = e.p;
    q.x < 0
};

atom alias_false_claim_top(e: Ev, x: u64) -> bool
requires: match e {
    Ev::Move(p) => p.x == x && p.inner.x == x,
    _ => false
} && x == 0 - 1;
ensures: result == true;
body: {
    let q = e.p;
    q.x < 0
};

atom alias_positive_nested(e: Ev, x: u64) -> bool
requires: match e {
    Ev::Move(p) => p.x == x && p.inner.x == x,
    _ => false
} && x == 0 - 1;
ensures: result == false;
body: {
    let i = e.p.inner;
    i.x < 0
};

atom alias_false_claim_nested(e: Ev, x: u64) -> bool
requires: match e {
    Ev::Move(p) => p.x == x && p.inner.x == x,
    _ => false
} && x == 0 - 1;
ensures: result == true;
body: {
    let i = e.p.inner;
    i.x < 0
};
