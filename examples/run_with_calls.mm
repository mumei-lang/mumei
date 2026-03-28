// P7-B: Binary execution demo with function calls
// Run with: mumei run examples/run_with_calls.mm

atom add(a: i64, b: i64)
    requires: a >= 0 && b >= 0;
    ensures: result == a + b;
    body: a + b;

atom double(n: i64)
    requires: n >= 0;
    ensures: result == n * 2;
    body: n + n;

atom main()
    requires: true;
    ensures: result >= 0;
    body: {
        let sum = add(3, 7);
        double(sum)
    }
