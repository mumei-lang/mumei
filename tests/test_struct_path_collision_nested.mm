struct NestedLeaf {
    x: i64
}

struct NestedCollision {
    child: NestedLeaf,
    child_x: i64
}

struct Holder {
    wrapped: NestedCollision
}

atom nested_path_collision(value: Holder) -> i64
    requires: true;
    ensures: true;
    body: 0
