resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;

atom reacquire()
    requires: true;
    ensures: result == 5;
    body: {
        acquire counter {
            counter.value = 5;
            0
        };
        acquire counter {
            counter.value
        }
    };
