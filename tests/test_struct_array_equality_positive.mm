struct P {
    x: i64
}

atom unmodified_struct_arrays_remain_equal(ps: [P], qs: [P]) -> bool
requires: len(ps) == len(qs) && ps == qs;
ensures: ps == qs;
body: true;
