// Escrow ownership protocol: a transfer may only be released after the
// counterparty has accepted, encoded as temporal effect transitions.
// expected: PASS

effect Escrow
    states: [Idle, Offered, Accepted, Released];
    initial: Idle;
    transition offer: Idle -> Offered;
    transition accept: Offered -> Accepted;
    transition release: Accepted -> Released;
    transition withdraw: Offered -> Idle;

atom offer_then_accept(new_owner: i64)
    effects: [Escrow];
    effect_pre: { Escrow: Idle };
    effect_post: { Escrow: Accepted };
    requires: new_owner >= 0;
    ensures: result == new_owner;
    body: {
        perform Escrow.offer;
        perform Escrow.accept;
        new_owner
    }

atom accept_then_release(new_owner: i64)
    effects: [Escrow];
    effect_pre: { Escrow: Offered };
    effect_post: { Escrow: Released };
    requires: new_owner >= 0;
    ensures: result == new_owner;
    body: {
        perform Escrow.accept;
        perform Escrow.release;
        new_owner
    }

atom withdraw_offer(new_owner: i64)
    effects: [Escrow];
    effect_pre: { Escrow: Offered };
    effect_post: { Escrow: Idle };
    requires: new_owner >= 0;
    ensures: result == new_owner;
    body: {
        perform Escrow.withdraw;
        new_owner
    }
