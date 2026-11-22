type Positive = i64 where v > 0;

struct P {
    x: Positive,
    y: i64
}

atom break_field_contract(ps: [P]) -> bool
requires: len(ps) > 0 && ps[0].x > 0;
ensures: result == true;
body: {
    ps[0].x = -1;
    true
};
