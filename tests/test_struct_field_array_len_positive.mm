struct Box {
    values: [i64]
}

struct Inner {
    values: [i64]
}

struct Outer {
    inner: Inner
}

atom len_same_field_ok(b: Box) -> bool
    requires: len(b.values) > 0;
    ensures: result == true;
    body: len(b.values) > 0

atom len_let_alias_ok(b: Box) -> bool
    requires: len(b.values) > 0;
    ensures: result == true;
    body: {
        let v = b.values;
        len(v) > 0
    }

atom read_let_alias_ok(b: Box) -> i64
    requires: len(b.values) > 2;
    ensures: true;
    body: {
        let v = b.values;
        v[1]
    }

atom len_after_field_array_assign(b: Box, xs: [i64]) -> bool
    requires: true;
    ensures: result == true;
    body: {
        let source_len = len(xs);
        b.values = xs;
        len(b.values) == source_len
    }

atom field_len_callee(p: Box) -> bool
    requires: len(p.values) > 0;
    ensures: result == true;
    body: true

atom field_len_call(b: Box) -> bool
    requires: len(b.values) > 0;
    ensures: result == true;
    body: field_len_callee(b)

atom nested_field_len_alias_ok(b: Outer) -> bool
    requires: len(b.inner.values) > 0;
    ensures: result == true;
    body: len(b.inner.values) > 0
