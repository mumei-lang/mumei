// Counterexample case: an exclusive resource is claimed twice by the same atom,
// a data race risk that the resource safety check must reject.
// expected: FAIL

resource shared_buffer priority: 1 mode: exclusive;

atom claim_buffer_twice(x: i64)
resources: [shared_buffer, shared_buffer];
requires: x >= 0;
ensures: result == x;
body: {
    acquire shared_buffer {
        x
    }
};
