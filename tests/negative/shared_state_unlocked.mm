resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;

atom unlocked()
    requires: true;
    ensures: result == 1;
    body: counter.value = 1;
