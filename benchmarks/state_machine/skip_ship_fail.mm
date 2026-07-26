// Counterexample case: the atom claims to reach Shipped directly from Created
// without performing `ship`, which is not an allowed transition.
// expected: FAIL

effect Order
    states: [Created, Paid, Shipped, Delivered, Cancelled];
    initial: Created;
    transition pay: Created -> Paid;
    transition ship: Paid -> Shipped;
    transition deliver: Shipped -> Delivered;
    transition cancel: Created -> Cancelled;

atom deliver_without_shipping(amount: i64)
    effects: [Order];
    effect_pre: { Order: Created };
    effect_post: { Order: Delivered };
    requires: amount > 0;
    ensures: result == amount;
    body: {
        perform Order.pay;
        perform Order.deliver;
        amount
    }
