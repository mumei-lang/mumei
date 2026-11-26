struct UInner {
    x: u64,
    y: u64
}

struct UP {
    inner: UInner,
    z: i64
}

enum UEv {
    Move(p: UP),
    Stop
}

atom u64_struct_payload_compare(e: UEv, x: u64) -> bool
requires: match e {
    UEv::Move(p) => p.inner.x == x,
    _ => false
} && x == 0 - 1;
ensures: result == false;
body: {
    let q = e.p;
    q.inner.x < 0
};
