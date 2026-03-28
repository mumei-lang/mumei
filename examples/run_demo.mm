// P7-B: Simple binary execution demo
// Run with: mumei run examples/run_demo.mm

atom main()
    requires: true;
    ensures: result >= 0;
    body: {
        let x = 5;
        let y = 10;
        x + y
    }
