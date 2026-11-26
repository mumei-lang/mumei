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

struct RecursiveAmbiguous {
    a: RecursiveAmbiguous,
    aa: RecursiveAmbiguous,
    value: i64
}

atom recursive_ambiguous_slot_search(q: RecursiveAmbiguous) -> i64
requires: true;
ensures: result == 1;
body: {
    let q_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_a_z = 1;
    1
};
