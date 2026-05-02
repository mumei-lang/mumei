// Negative test: Ownership accept without a pending transfer
// Expected to FAIL verification.
// Expected: InvalidPreState because accept_transfer requires
// Ownership:PendingTransfer but hostile_takeover starts from Ownership:Idle.

import "std/ownership" as ownership;

atom hostile_takeover(new_owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: Idle };
    effect_post: { Ownership: Transferred };
    requires: new_owner >= 0;
    ensures: result == new_owner;
    body: {
        ownership::accept_transfer(new_owner);
        new_owner
    };
