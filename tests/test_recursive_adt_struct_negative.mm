struct Node {
    value: i64,
    next: K,
}

enum K {
    Leaf,
    Wrap(value: Node),
}

atom unsupported_struct_cycle() -> K
    requires: true;
    ensures: true;
    body: K::Wrap(Node { value: 0, next: K::Leaf })
