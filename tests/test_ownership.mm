// =============================================================
// tests/test_ownership.mm — Ownership Transfer Protocol E2E Test
// =============================================================
// Integration test for std/ownership.mm stateful effect contracts.
// Usage: mumei check tests/test_ownership.mm
// Verification: mumei verify tests/test_ownership.mm

import "std/ownership" as ownership;

// Valid: Idle -> PendingTransfer -> Transferred
atom test_propose_accept(new_owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: Idle };
    effect_post: { Ownership: Transferred };
    requires: new_owner >= 0;
    ensures: result == new_owner;
    body: {
        ownership::propose_transfer(new_owner);
        ownership::accept_transfer(new_owner);
        new_owner
    };

// Valid: Idle -> PendingTransfer -> Idle
atom test_propose_cancel(owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: Idle };
    effect_post: { Ownership: Idle };
    requires: owner >= 0;
    ensures: result == owner;
    body: {
        ownership::propose_transfer(owner);
        ownership::cancel_transfer(owner);
        owner
    };

// Valid: std atom composes propose -> accept
atom test_full_transfer(new_owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: Idle };
    effect_post: { Ownership: Transferred };
    requires: new_owner >= 0;
    ensures: result == new_owner;
    body: {
        ownership::full_transfer(new_owner)
    };

// Valid: std atom composes propose -> cancel
atom test_propose_and_cancel(owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: Idle };
    effect_post: { Ownership: Idle };
    requires: owner >= 0;
    ensures: result == owner;
    body: {
        ownership::propose_and_cancel(owner)
    };
