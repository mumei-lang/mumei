struct CollisionInner {
    x: i64
}

struct CollisionP {
    inner: CollisionInner
}

enum CollisionEv {
    Move(p: CollisionP),
    Stop
}

struct SelfReferenceP {
    x: i64
}

atom local_before_struct_slot_collision(e: CollisionEv) -> i64
requires: e.tag == 0 && e.p.inner.x == 0;
ensures: result == 0;
body: {
    let q_inner_x = 1;
    let q = e.p;
    q_inner_x
};

atom local_after_struct_slot_collision(e: CollisionEv) -> i64
requires: e.tag == 0;
ensures: result == 1;
body: {
    let q = e.p;
    let q_inner_x = 1;
    q_inner_x
};

atom self_referential_projection_slot(p: SelfReferenceP) -> i64
requires: p.x == 1;
ensures: result == 1;
body: {
    let p_x = p;
    0
};

struct ArrayItem {
    x: i64
}

struct ArrayP {
    items: [ArrayItem]
}

atom array_element_field_name_is_not_a_projection_slot(items: [ArrayItem]) -> i64
requires: true;
ensures: result == 0;
body: {
    let q = ArrayP { items: items };
    let q_items_x = 1;
    0
};
