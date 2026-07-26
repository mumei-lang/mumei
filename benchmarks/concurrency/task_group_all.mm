// Structured concurrency: `task` / `task_group:all` join semantics.
// expected: PASS

atom spawn_single_task(n: i64)
requires: n >= 0;
ensures: result == n;
body: {
    task { n }
};

atom join_all_last_result(a: i64, b: i64)
requires: a >= 0 && b >= 0;
ensures: result == b;
body: {
    task_group:all {
        task { a };
        task { b }
    }
};

atom nested_task_groups(a: i64, b: i64, c: i64)
requires: a >= 0 && b >= 0 && c >= 0;
ensures: result == c;
body: {
    task_group:all {
        task { a };
        task {
            task_group:all {
                task { b };
                task { c }
            }
        }
    }
};

atom fan_out_bounded_work(n: i64)
requires: n >= 0 && n <= 1000;
ensures: result >= 0 && result <= 1000;
body: {
    task_group:all {
        task { n };
        task { n }
    }
};
