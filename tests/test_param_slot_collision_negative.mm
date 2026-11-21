struct SlotInner {
    values: [i64]
}

struct SlotOuter {
    inner: SlotInner
}

struct ScalarP {
    x: i64
}

struct PathLeaf {
    x: i64
}

struct PathP {
    a_b: PathLeaf
}

struct PathQ {
    b_x: i64
}

atom overlapping_struct_slots(outer: SlotOuter, outer_inner: SlotInner) -> i64
requires: true;
ensures: true;
body: len(outer.inner.values) - len(outer_inner.values)

atom scalar_parameter_slot_false_claim(p: ScalarP, p_x: i64) -> i64
requires: true;
ensures: result == p_x;
body: p.x

atom nested_struct_slot_collision(p: PathP, p_a: PathQ) -> i64
requires: true;
ensures: result == 0;
body: 0

struct Item {
    x: i64
}

atom real_struct_slot_collision(item: Item, item_x: i64) -> i64
requires: true;
ensures: result == item_x;
body: item.x

struct RecursiveParam {
    x: i64,
    a_x: i64,
    r: RecursiveParam
}

atom recursive_parameter_projection_overlap(
    p: RecursiveParam,
    p_r_a: RecursiveParam
) -> i64
requires: true;
ensures: result == 0;
body: 0

atom let_name_collides_parameter_slot(p: ScalarP) -> i64
requires: true;
ensures: result == 1;
body: {
    let p_x = 1;
    p.x
}
