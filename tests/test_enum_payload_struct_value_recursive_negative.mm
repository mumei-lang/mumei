struct Node {
    value: i64,
    next: Node
}

enum RecursiveEv {
    Move(node: Node),
    Stop
}

atom bad_recursive_payload(e: RecursiveEv) -> i64
requires: match e {
    RecursiveEv::Move(_) => true,
    _ => false
};
ensures: result == 0;
body: {
    let q = e.node;
    q.value
};
