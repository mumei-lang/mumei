// RegTech: every customer category (including PEP) must be classified, and
// transaction limits must hold for every risk level.
// expected: PASS

enum CustomerType {
    Individual,
    Corporate,
    Government,
    PEP
}

atom classify_all_customer_types(customer_type: CustomerType)
    requires: true;
    ensures: result >= 0 && result <= 3;
    body: {
        match customer_type {
            Individual => 0,
            Corporate => 1,
            Government => 0,
            PEP => 3
        }
    }

atom limit_for_risk_level(risk_level: i64)
    requires: risk_level >= 0 && risk_level <= 3;
    ensures: result > 0 && result <= 1000000;
    body: {
        match risk_level {
            0 => 1000000,
            1 => 100000,
            2 => 10000,
            3 => 1000,
            _ => 1000
        }
    }

atom transaction_within_limit(amount: i64, limit: i64)
    requires: amount >= 0 && limit > 0 && amount <= limit;
    ensures: result == amount && result <= limit;
    body: amount;

atom all_transactions_within_limit(amounts: [i64], n: i64, limit: i64)
    requires: n >= 0 && len(amounts) >= n && limit > 0
        && forall(i, 0, n, amounts[i] >= 0 && amounts[i] <= limit);
    ensures: result >= 0;
    body: {
        let count = 0;
        let i = 0;
        while i < n
        invariant: i >= 0 && i <= n && count >= 0
        decreases: n - i
        {
            count = count + 1;
            i = i + 1;
        };
        count
    }
