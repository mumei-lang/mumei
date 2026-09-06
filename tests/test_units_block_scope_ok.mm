type Usd = i64 unit USD;
type Jpy = i64 unit JPY;

atom jpy_value() -> Jpy
    requires: true;
    ensures: result == 1;
    body: 1;

atom block_scope_after_while(amount: Usd) -> Usd
    requires: true;
    ensures: result == amount;
    body: {
        while false
        invariant: true
        decreases: 0
        {
            let amount = jpy_value();
        };
        amount
    };
