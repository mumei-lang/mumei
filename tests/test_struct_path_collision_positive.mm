struct Inner {
    x: i64
}

struct Safe {
    inner: Inner,
    inner_y: i64,
    inner_values: [i64]
}

struct Recursive {
    next: Recursive
}

struct ArrayRecursive {
    children: [ArrayRecursive]
}

struct RecursiveWide {
    left: RecursiveWide,
    right: RecursiveWide,
    up: RecursiveWide,
    down: RecursiveWide
}

atom safe_struct_paths(
    value: Safe,
    node: Recursive,
    array_node: ArrayRecursive
) -> i64
    requires: true;
    ensures: true;
    body: 0
