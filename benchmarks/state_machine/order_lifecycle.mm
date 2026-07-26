// Finite state machine transition invariants for an order lifecycle,
// enforced through temporal effect states.
// expected: PASS

effect Order
    states: [Created, Paid, Shipped, Delivered, Cancelled];
    initial: Created;
    transition pay: Created -> Paid;
    transition ship: Paid -> Shipped;
    transition deliver: Shipped -> Delivered;
    transition cancel: Created -> Cancelled;

atom pay_then_ship(amount: i64)
    effects: [Order];
    effect_pre: { Order: Created };
    effect_post: { Order: Shipped };
    requires: amount > 0;
    ensures: result == amount;
    body: {
        perform Order.pay;
        perform Order.ship;
        amount
    }

atom full_fulfillment(amount: i64)
    effects: [Order];
    effect_pre: { Order: Created };
    effect_post: { Order: Delivered };
    requires: amount > 0;
    ensures: result == amount;
    body: {
        perform Order.pay;
        perform Order.ship;
        perform Order.deliver;
        amount
    }

atom cancel_before_payment(amount: i64)
    effects: [Order];
    effect_pre: { Order: Created };
    effect_post: { Order: Cancelled };
    requires: amount > 0;
    ensures: result == amount;
    body: {
        perform Order.cancel;
        amount
    }

atom deliver_from_paid(amount: i64)
    effects: [Order];
    effect_pre: { Order: Paid };
    effect_post: { Order: Delivered };
    requires: amount > 0;
    ensures: result == amount;
    body: {
        perform Order.ship;
        perform Order.deliver;
        amount
    }
