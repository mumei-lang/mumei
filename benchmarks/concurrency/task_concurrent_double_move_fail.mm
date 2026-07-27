// Counterexample case: the same buffer is consumed by two concurrent sibling
// tasks — a concurrent double move (double free) that sequential MIR move
// analysis does not model.
// expected: FAIL

atom take_buffer(buf: [i64])
requires: len(buf) >= 0;
consume buf;
ensures: result >= 0;
body: len(buf);

atom move_buffer_into_two_tasks(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {
    task_group:all {
        task { take_buffer(buf) };
        task { take_buffer(buf) }
    }
};
