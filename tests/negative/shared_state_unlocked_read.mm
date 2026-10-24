resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;

atom read_unlocked()
    requires: true;
    ensures: result >= 0;
    body: {
        counter.value
    };
