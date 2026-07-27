// Counterexample case: a buffer is moved into a child task and then read again
// by the parent after the group joins.
// expected: FAIL

atom take_buffer(buf: [i64])
requires: len(buf) >= 0;
consume buf;
ensures: result >= 0;
body: len(buf);

atom read_buffer_after_task_moved_it(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {
    task_group:all {
        task { take_buffer(buf) };
        task { 0 }
    };
    len(buf)
};
