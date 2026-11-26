struct ArrayInner {
    values: [i64]
}

struct ArrayS {
    inner: ArrayInner,
    inner_values: [i64]
}

atom array_path_false_claim(s: ArrayS) -> i64
    requires: true;
    ensures: len(s.inner.values) == len(s.inner_values);
    body: 0
