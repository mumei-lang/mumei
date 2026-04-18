// =============================================================
// tests/test_safe_queue.mm — SafeQueue E2E Test
// =============================================================
// Integration test for std/container/safe_queue.mm module.
// Usage: mumei check tests/test_safe_queue.mm
import "std/container/safe_queue" as queue;

// Test: enqueue increases length by 1
atom test_enqueue(cap: i64)
    requires: cap > 0;
    ensures: result == 1;
    body: {
        queue::enqueue(0, cap)
    };

// Test: dequeue decreases length by 1
atom test_dequeue()
    requires: true;
    ensures: result == 4;
    body: {
        queue::dequeue(5)
    };

// Test: empty check on empty queue returns 1
atom test_queue_is_empty()
    requires: true;
    ensures: result == 1;
    body: {
        queue::queue_is_empty(0)
    };

// Test: full check when len == cap returns 1
atom test_queue_is_full()
    requires: true;
    ensures: result == 1;
    body: {
        queue::queue_is_full(10, 10)
    };

// Test: batch enqueue adds count elements
atom test_batch_enqueue(cap: i64)
    requires: cap >= 5;
    ensures: result == 5;
    body: {
        queue::batch_enqueue(0, cap, 5)
    };

// Test: enqueue then dequeue preserves original length
atom test_enqueue_dequeue_identity(cap: i64)
    requires: cap > 0;
    ensures: result == 0;
    body: {
        let after_enqueue = queue::enqueue(0, cap);
        queue::dequeue(after_enqueue)
    };
