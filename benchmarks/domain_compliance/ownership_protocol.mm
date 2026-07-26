// Ownership transfer protocol: ownership only changes hands through
// propose -> accept, and cancel restores the previous owner.
// expected: PASS

effect Ownership
    states: [Idle, PendingTransfer, Transferred];
    initial: Idle;
    transition propose: Idle -> PendingTransfer;
    transition accept: PendingTransfer -> Transferred;
    transition cancel: PendingTransfer -> Idle;

atom propose_transfer(new_owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: Idle };
    effect_post: { Ownership: PendingTransfer };
    requires: new_owner >= 0;
    ensures: result == new_owner;
    body: {
        perform Ownership.propose;
        new_owner
    }

atom full_transfer(new_owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: Idle };
    effect_post: { Ownership: Transferred };
    requires: new_owner >= 0;
    ensures: result == new_owner;
    body: {
        perform Ownership.propose;
        perform Ownership.accept;
        new_owner
    }

atom cancelled_transfer_keeps_owner(current_owner: i64, new_owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: Idle };
    effect_post: { Ownership: Idle };
    requires: current_owner >= 0 && new_owner >= 0 && current_owner != new_owner;
    ensures: result == current_owner;
    body: {
        perform Ownership.propose;
        perform Ownership.cancel;
        current_owner
    }
