resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;

atom broken()
    requires: true;
    ensures: result == 0;
    body: {
        acquire counter {
            counter.value = counter.value - 1;
        }
        0
    };
