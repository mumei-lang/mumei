struct RecursiveWide {
    left: RecursiveWide,
    right: RecursiveWide,
    up: RecursiveWide,
    down: RecursiveWide
}

enum RecursiveWideEv {
    Hold(node: RecursiveWide),
    Empty
}

atom recursive_slot_walk(e: RecursiveWideEv) -> i64
requires: true;
ensures: result == 0;
body: {
    let q = e.node;
    let q = e.node;
    0
};
