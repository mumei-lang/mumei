effect RunwayAllocation
    states: [Idle, Ordered, Allocated];
    initial: Idle;
    transition order: Idle -> Ordered;
    transition allocate: Ordered -> Allocated;

resource runway_primary priority: 1 mode: exclusive;
resource runway_secondary priority: 2 mode: exclusive;

atom allocate_runway(flight: i64, runway1: i64, runway2: i64, lock_state: i64) -> i64
    effects: [RunwayAllocation]
    resources: [runway_primary, runway_secondary]
    requires: flight >= 0 && runway1 >= 0 && runway2 >= 0 && runway1 != runway2 && runway1 < runway2;
    ensures: result != 0 && result == runway1 + runway2;
    body: {
        perform RunwayAllocation.order;
        acquire runway_primary {
            acquire runway_secondary {
                perform RunwayAllocation.allocate;
                runway1 + runway2
            }
        }
    }
