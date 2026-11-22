struct N {
    v: i64,
    k: K
}

enum K {
    Leaf,
    Node(n: N)
}

atom f(k: K) -> i64
requires: true;
ensures: true;
body: {
    match k {
        K::Node(n) => n.v,
        K::Leaf => 0
    }
};
