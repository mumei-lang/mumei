enum ScalarArrayE {
    D(xs: [i64]),
    N
}

enum NestedScalarArrayE {
    D(inner: ScalarArrayE),
    N
}

atom scalar_array_payload_empty_equality(a: [i64], b: [i64]) -> bool
requires: len(a) == 0 && len(b) == 0;
ensures: result == true;
body: ScalarArrayE::D(a) == ScalarArrayE::D(b);

atom nested_scalar_array_payload_empty_equality(a: [i64], b: [i64]) -> bool
requires: len(a) == 0 && len(b) == 0;
ensures: result == true;
body: NestedScalarArrayE::D(ScalarArrayE::D(a)) == NestedScalarArrayE::D(ScalarArrayE::D(b));
