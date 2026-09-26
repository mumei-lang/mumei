resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;

atom bump()
    requires: true;
    ensures: result == 1;
    body: {
        acquire counter {
            counter.value = counter.value + 1;
        }
        1
    };

resource shared_counter { value: i64, ready: bool } priority: 2 mode: shared invariant: shared_counter.value >= 0;

atom read_shared()
    requires: true;
    ensures: result >= 0;
    body: {
        acquire shared_counter {
            if shared_counter.ready { 1 } else { 0 }
        }
    };

atom sibling_bumps()
    requires: true;
    ensures: result >= 0;
    body: {
        task_group:all {
            task {
                acquire counter {
                    counter.value = counter.value + 1;
                }
                1
            };
            task {
                acquire counter {
                    counter.value = counter.value + 1;
                }
                1
            }
        }
    };
