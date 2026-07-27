// DeFi vault invariants: checks-effects-interactions ordering enforced as a
// temporal effect, plus bounded-integer accounting for deposits and withdrawals.
// expected: PASS

effect Vault
    states: [Idle, Checked, Effected, Interacted];
    initial: Idle;
    transition check: Idle -> Checked;
    transition update: Checked -> Effected;
    transition interact: Effected -> Interacted;

atom cei_compliant_withdraw(balance: i64, amount: i64)
    effects: [Vault];
    effect_pre: { Vault: Idle };
    effect_post: { Vault: Interacted };
    requires: balance >= 0 && balance <= 1000000000 && amount > 0 && balance >= amount;
    ensures: result == balance - amount && result >= 0;
    body: {
        perform Vault.check;
        perform Vault.update;
        perform Vault.interact;
        balance - amount
    }

atom guarded_state_update(balance: i64, amount: i64)
    effects: [Vault];
    effect_pre: { Vault: Checked };
    effect_post: { Vault: Effected };
    requires: balance >= 0 && balance <= 1000000000 && amount > 0 && balance >= amount;
    ensures: result == balance - amount;
    body: {
        perform Vault.update;
        balance - amount
    }

atom deposit_without_overflow(balance: i64, amount: i64)
    requires: balance >= 0 && balance <= 1000000000 && amount > 0 && amount <= 1000000000;
    ensures: result == balance + amount && result > balance && result <= 2000000000;
    body: balance + amount;

atom share_price_is_bounded(total_assets: i64, total_shares: i64)
    requires: total_assets >= 0 && total_assets <= 1000000 && total_shares >= 1 && total_shares <= 1000000;
    ensures: result >= 0 && result <= total_assets;
    body: total_assets / total_shares;
