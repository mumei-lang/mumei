struct Box {
    values: [i64]
}

struct P {
    x: i64
}

struct Item {
    x: i64
}

struct ArrayP {
    items: [Item]
}

atom distinct_box_parameters(b: Box, c: Box) -> i64
requires: true;
ensures: result == 0;
body: 0

atom noncolliding_prefix_parameters(p: P, pp: P) -> i64
requires: true;
ensures: result == p.x + pp.x;
body: p.x + pp.x

atom parameter_suffix_without_field(p: P, p_y: i64) -> i64
requires: true;
ensures: result == p_y;
body: p_y

atom array_element_suffix_is_not_a_projection(p: ArrayP, p_items_x: i64) -> i64
requires: true;
ensures: result == p_items_x;
body: p_items_x

atom array_parameter_suffix_is_not_a_projection(items: [Item], items_x: i64) -> i64
requires: true;
ensures: result == items_x;
body: items_x

atom local_array_of_structs() -> i64
requires: true;
ensures: result == 2;
body: {
    let xs = [Item { x: 1 }];
    let xs_x = 2;
    xs_x
}
