enum ScalarArrayE {
    D(xs: [i64]),
    N
}

atom scalar_array_payload_different_contents_reject_equality(xs: [i64], ys: [i64]) -> bool
requires: len(xs) == 1 && len(ys) == 1 && xs[0] != ys[0];
ensures: result == true;
body: ScalarArrayE::D(xs) == ScalarArrayE::D(ys);
