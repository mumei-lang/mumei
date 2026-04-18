// Verified State Machine Pattern — Invalid Transition
// This file should FAIL compilation: ship before pay is an invalid transition.

effect Order
    states: [Created, Paid, Shipped, Delivered, Cancelled];
    initial: Created;
    transition pay: Created -> Paid;
    transition ship: Paid -> Shipped;
    transition deliver: Shipped -> Delivered;
    transition cancel: Created -> Cancelled;

atom invalid_order(x: i64)
    effects: [Order];
    effect_pre: { Order: Created };
    effect_post: { Order: Shipped };
    requires: x > 0;
    ensures: result >= 0;
    body: {
        perform Order.ship;
        x
    }
