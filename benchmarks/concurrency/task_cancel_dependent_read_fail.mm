// Counterexample case: the parent reads a variable written by a
// `task_group:any` child. The losing child is cancelled, so the write may never
// have happened — the value the parent observes is cancellation-dependent.
// expected: FAIL

atom read_cancellable_write(n: i64)
requires: n >= 0 && n <= 100;
ensures: result >= 0;
body: {
    let total = n;
    task_group:any {
        task {
            total = total + 1;
            total
        };
        task { n }
    };
    total
};
