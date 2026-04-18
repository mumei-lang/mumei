// P7-B: Binary execution demo with struct and impl block
// Run with: mumei run examples/run_with_impl.mm

struct Counter {
    value: i64,
}

impl Counter {
    atom get_value(v: i64)
        requires: v >= 0;
        ensures: result == v;
        body: v;

    atom increment(v: i64)
        requires: v >= 0;
        ensures: result == v + 1;
        body: v + 1;
}

atom main()
    requires: true;
    ensures: result >= 0;
    body: {
        let v = Counter::get_value(10);
        Counter::increment(v)
    }
