// Financial RTGS settlement: total balance is conserved across a transfer and
// the settlement lifecycle follows Pending -> Validated -> Settled.
// expected: PASS

effect Settlement
    states: [Pending, Validated, Settled];
    initial: Pending;
    transition validate: Pending -> Validated;
    transition settle: Validated -> Settled;
    transition reject: Pending -> Pending;

atom validate_sufficient_funds(sender_balance: i64, amount: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Pending };
    effect_post: { Settlement: Validated };
    requires: sender_balance >= 0 && sender_balance <= 1000000000 && amount > 0 && sender_balance >= amount;
    ensures: result == sender_balance - amount && result >= 0;
    body: {
        perform Settlement.validate;
        sender_balance - amount
    }

atom settle_conserves_total(sender_balance: i64, receiver_balance: i64, amount: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Validated };
    effect_post: { Settlement: Settled };
    requires: sender_balance >= 0 && sender_balance <= 1000000000
        && receiver_balance >= 0 && receiver_balance <= 1000000000
        && amount > 0 && sender_balance >= amount;
    ensures: result == sender_balance + receiver_balance;
    body: {
        perform Settlement.settle;
        let new_sender = sender_balance - amount;
        let new_receiver = receiver_balance + amount;
        new_sender + new_receiver
    }

atom reject_insufficient_funds(sender_balance: i64, amount: i64)
    effects: [Settlement];
    effect_pre: { Settlement: Pending };
    effect_post: { Settlement: Pending };
    requires: sender_balance >= 0 && amount > 0 && sender_balance < amount;
    ensures: result == sender_balance;
    body: {
        perform Settlement.reject;
        sender_balance
    }

atom queue_total_is_nonnegative(balances: [i64], n: i64)
    requires: n >= 0 && len(balances) >= n && forall(i, 0, n, balances[i] >= 0);
    ensures: result >= 0;
    body: {
        let total = 0;
        let i = 0;
        while i < n
        invariant: i >= 0 && i <= n && total >= 0
        decreases: n - i
        {
            total = total + balances[i];
            i = i + 1;
        };
        total
    }
