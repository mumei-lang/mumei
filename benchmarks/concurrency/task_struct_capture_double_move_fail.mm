// Counterexample case: a struct capture (non-i64, Move movability) is consumed
// by two concurrent sibling tasks — a concurrent double move.
// expected: FAIL

struct Point {
    x: i64,
    y: i64
}

atom take_point(p: Point)
requires: p.x >= 0;
consume p;
ensures: result >= 0;
body: p.x;

atom move_point_into_two_tasks(p: Point)
requires: p.x >= 0;
ensures: result >= 0;
body: {
    task_group:all {
        task { take_point(p) };
        task { take_point(p) }
    }
};
