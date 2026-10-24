resource counter { value: i64 } priority: 1 mode: shared invariant: counter.value >= 0;

atom write_shared()
    requires: true;
    ensures: result == 1;
    body: {
        acquire counter {
            counter.value = 1;
        }
        1
    };
