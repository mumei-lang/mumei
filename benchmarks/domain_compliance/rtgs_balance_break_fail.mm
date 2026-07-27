// Counterexample case: the settlement credits the receiver without debiting the
// sender, so the total balance is not conserved.
// expected: FAIL

atom settle_loses_conservation(sender_balance: i64, receiver_balance: i64, amount: i64)
    requires: sender_balance >= 0 && sender_balance <= 1000000000
        && receiver_balance >= 0 && receiver_balance <= 1000000000
        && amount > 0 && sender_balance >= amount;
    ensures: result == sender_balance + receiver_balance;
    body: {
        let new_receiver = receiver_balance + amount;
        sender_balance + new_receiver
    }
