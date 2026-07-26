// Counterexample case: releasing escrow funds without going through `accept`.
// The transition Offered -> Released does not exist, so the temporal effect
// checker must reject this atom.
// expected: FAIL

effect Escrow
    states: [Idle, Offered, Accepted, Released];
    initial: Idle;
    transition offer: Idle -> Offered;
    transition accept: Offered -> Accepted;
    transition release: Accepted -> Released;

atom release_without_accept(new_owner: i64)
    effects: [Escrow];
    effect_pre: { Escrow: Offered };
    effect_post: { Escrow: Released };
    requires: new_owner >= 0;
    ensures: result == new_owner;
    body: {
        perform Escrow.release;
        new_owner
    }
