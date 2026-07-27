// Counterexample case: two sibling tasks of the same `task_group:all` write
// and read the same captured variable without synchronisation.
// expected: FAIL

atom race_on_shared_counter(n: i64)
requires: n >= 0 && n <= 100;
ensures: result >= 0;
body: {
    let counter = n;
    task_group:all {
        task {
            counter = counter + 1;
            counter
        };
        task {
            counter = counter + 2;
            counter
        }
    }
};
