resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 1;

atom initial()
    requires: true;
    ensures: result == 0;
    body: 0;
