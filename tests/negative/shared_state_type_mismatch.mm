resource counter { ready: bool } priority: 1 mode: exclusive invariant: counter.ready == false;

atom mismatch()
    requires: true;
    ensures: result == 0;
    body: {
        acquire counter {
            counter.ready = 42;
        }
        0
    };
