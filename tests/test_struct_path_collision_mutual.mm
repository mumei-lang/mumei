struct A {
    b: B,
    b_y: i64
}

struct B {
    y: i64,
    a: A
}

atom mutual_recursive_path_collision(value: A) -> i64
    requires: true;
    ensures: true;
    body: 0
