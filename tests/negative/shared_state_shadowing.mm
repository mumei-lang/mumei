resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;

atom shadowed()
    requires: true;
    ensures: result == 0;
    body: {
        let counter = 0;
        0
    };
