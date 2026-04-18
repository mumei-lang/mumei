// E2E test for Verified State Machine Pattern
// Run with: mumei verify tests/test_order_state_machine.mm

effect Order
    states: [Created, Paid, Shipped, Delivered, Cancelled];
    initial: Created;
    transition pay: Created -> Paid;
    transition ship: Paid -> Shipped;
    transition deliver: Shipped -> Delivered;
    transition cancel: Created -> Cancelled;

// Valid: Created -> Paid -> Shipped (standard fulfillment)
atom test_standard_fulfillment(x: i64)
    effects: [Order];
    effect_pre: { Order: Created };
    effect_post: { Order: Shipped };
    requires: x > 0;
    ensures: result == x;
    body: {
        perform Order.pay;
        perform Order.ship;
        x
    }

// Valid: Created -> Cancelled (order cancellation)
atom test_cancellation(x: i64)
    effects: [Order];
    effect_pre: { Order: Created };
    effect_post: { Order: Cancelled };
    requires: x > 0;
    ensures: result == x;
    body: {
        perform Order.cancel;
        x
    }

// Valid: Paid -> Shipped -> Delivered (delivery from paid state)
atom test_delivery(x: i64)
    effects: [Order];
    effect_pre: { Order: Paid };
    effect_post: { Order: Delivered };
    requires: x > 0;
    ensures: result == x;
    body: {
        perform Order.ship;
        perform Order.deliver;
        x
    }
