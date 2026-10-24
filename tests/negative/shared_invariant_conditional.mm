resource counter { value: i64 } priority: 1 mode: exclusive invariant: counter.value >= 0;

atom conditional_break(flag: bool)
    requires: true;
    ensures: result == 0;
    body: {
        acquire counter {
            if flag {
                counter.value = counter.value - 1;
            } else {
                counter.value = counter.value;
            }
        }
        0
    };
