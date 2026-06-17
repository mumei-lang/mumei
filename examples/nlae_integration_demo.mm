// P9-G: NLAE ecosystem integration demo.
//
// Intentional vault bug: the withdraw path adds the amount back to the
// balance. Running `mumei verify --emit loss-vector examples/nlae_integration_demo.mm`
// demonstrates the P9-E structured Loss Vector consumed by mumei-agent.

atom nlae_vault_withdraw_amount_nonnegative_bound(balance: i64, amount: i64)
    requires: balance >= 0 && amount >= 0 && amount <= balance;
    ensures: result <= balance && result >= 0;
    body: balance + amount;

atom nlae_vault_no_negative_withdraw(balance: i64, amount: i64)
    requires: balance >= 0 && amount >= 0 && amount <= balance;
    ensures: result == balance - amount;
    body: balance + amount;
