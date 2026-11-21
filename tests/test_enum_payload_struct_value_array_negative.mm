struct ArrayP {
    values: [i64]
}

enum ArrayEv {
    Move(p: ArrayP),
    Stop
}

atom bad_array_payload(e: ArrayEv) -> i64
requires: match e {
    ArrayEv::Move(_) => true,
    _ => false
};
ensures: result == 1;
body: {
    let q = e.p;
    let values = q.values;
    values[0]
};
