struct DeepLeaf {
    c: i64
}

struct DeepA {
    b: DeepLeaf,
    b_c: i64
}

struct DeepOuterLeaf {
    c: i64
}

struct DeepS {
    a: DeepA,
    a_b: DeepOuterLeaf
}

atom deep_path_collision(s: DeepS) -> i64
    requires: true;
    ensures: true;
    body: 0
