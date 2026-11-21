struct Box {
    values: [i64]
}

struct Inner {
    values: [i64]
}

struct Outer {
    inner: Inner
}

atom len_confusion(b: Box, c: Box) -> i64
    requires: len(b.values) > 5;
    ensures: result > 5;
    body: len(c.values)

atom len_confusion_eq(b: Box, c: Box) -> bool
    requires: true;
    ensures: len(b.values) == len(c.values);
    body: true

atom len_nested_confusion(b: Outer, c: Outer) -> i64
    requires: len(b.inner.values) > 5;
    ensures: result > 5;
    body: len(c.inner.values)

atom len_if_branch_confusion(flag: bool, b: Box, c: Box) -> i64
    requires: len(if flag { b.values } else { b.values }) > 5;
    ensures: result > 5;
    body: len(if flag { c.values } else { c.values })

atom len_match_branch_confusion(tag: i64, b: Box, c: Box) -> i64
    requires: len(match tag { 0 => b.values, _ => b.values }) > 5;
    ensures: result > 5;
    body: len(match tag { 0 => c.values, _ => c.values })

atom len_field_length_stays_with_original_root(xs: [i64], b: Box) -> bool
    requires: len(xs) == 3;
    ensures: len(b.values) == 1;
    body: {
        b.values = xs;
        xs = [1];
        true
    }

atom len_field_conflicts_with_parameter_name(b: Box, len_b_values: i64) -> bool
    requires: true;
    ensures: len(b.values) == len_b_values;
    body: true

atom len_plain_array_conflicts_with_parameter_name(xs: [i64], len_xs: i64) -> i64
    requires: true;
    ensures: len(xs) == len_xs;
    body: 0

atom len_plain_array_false_claim(xs: [i64], len_xs: i64) -> i64
    requires: len_xs == 7;
    ensures: len(xs) == 7;
    body: 0
