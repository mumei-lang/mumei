struct Node {
    value: i64,
    next: Node
}

enum RecursiveEv {
    Move(node: Node),
    Stop
}

atom bad_recursive_payload(e: RecursiveEv) -> i64
requires: true;
ensures: result == 0;
body: e.node.value;
