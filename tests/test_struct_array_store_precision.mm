struct P {
    x: i64
}

atom field_store_preserves_other_index(ps: [P]) -> bool
requires: len(ps) > 1 && ps[1].x == 1;
ensures: 10 / ps[1].x == 10;
body: {
    ps[0].x = 9;
    true
};

atom whole_element_store_preserves_other_index(ps: [P]) -> bool
requires: len(ps) > 1 && ps[1].x == 1;
ensures: 10 / ps[1].x == 10;
body: {
    ps[0] = P { x: 9 };
    true
};
