// Structured concurrency ownership: captures that are safe under concurrent
// execution and under `task_group:any` cancellation.
// expected: PASS

atom read_shared_capture_in_siblings(a: i64, b: i64)
requires: a >= 0 && b >= 0;
ensures: result >= 0;
body: {
    task_group:all {
        task { a + b };
        task { a + b }
    }
};

atom per_task_local_writes(n: i64)
requires: n >= 0 && n <= 100;
ensures: result >= 0;
body: {
    task_group:all {
        task {
            let acc = n;
            acc = acc + 1;
            acc
        };
        task {
            let acc = n;
            acc = acc + 2;
            acc
        }
    }
};

atom move_buffer_into_single_task(buf: [i64])
requires: len(buf) >= 1;
ensures: result >= 0;
body: {
    task_group:all {
        task {
            let owned = buf;
            len(owned)
        };
        task { 0 }
    }
};

atom any_result_is_not_read_back(a: i64, b: i64)
requires: a >= 0 && b >= 0;
ensures: result >= 0;
body: {
    task_group:any {
        task { a };
        task { b }
    }
};
