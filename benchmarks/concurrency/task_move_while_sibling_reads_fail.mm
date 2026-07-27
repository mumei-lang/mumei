// Counterexample case: one child task consumes the captured buffer while a
// concurrent sibling still reads it.
// expected: FAIL

atom take_buffer(buf: [i64])
requires: len(buf) >= 0;
consume buf;
ensures: result >= 0;
body: len(buf);

atom move_while_sibling_reads(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {
    task_group:all {
        task { take_buffer(buf) };
        task { len(buf) }
    }
};
