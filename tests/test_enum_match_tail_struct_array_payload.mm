struct Pt {
    x: i64
}

enum DirectArrayE {
    D(ps: [Pt]),
    N
}

enum Selector {
    Only
}

atom block_tail_match_struct_array_payload() -> i64
requires: true;
ensures: result == 9;
body: {
    let e = DirectArrayE::D({
        let marker = 0;
        match Selector::Only {
            Selector::Only => [Pt { x: 9 }]
        }
    });
    match e {
        DirectArrayE::D(ps) => ps[0].x,
        DirectArrayE::N => 0
    }
};
