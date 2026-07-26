// Counterexample case: the external interaction happens before the internal
// balance update (checks-interactions-effects), which is the reentrancy hazard
// the Vault state machine forbids.
// expected: FAIL

effect Vault
    states: [Idle, Checked, Effected, Interacted];
    initial: Idle;
    transition check: Idle -> Checked;
    transition update: Checked -> Effected;
    transition interact: Effected -> Interacted;

atom interact_before_state_update(balance: i64, amount: i64)
    effects: [Vault];
    effect_pre: { Vault: Idle };
    effect_post: { Vault: Interacted };
    requires: balance >= 0 && balance <= 1000000000 && amount > 0 && balance >= amount;
    ensures: result == balance - amount;
    body: {
        perform Vault.check;
        perform Vault.interact;
        balance - amount
    }
