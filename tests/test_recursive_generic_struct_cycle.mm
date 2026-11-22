struct Box<T> {
    value: T
}

struct Node {
    value: Box<Node>
}

atom read(value: Node) -> i64
requires: true;
ensures: result == 0;
body: {
    0
};
