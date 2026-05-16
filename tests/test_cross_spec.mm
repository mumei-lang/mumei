atom validate_balance(x: i64) -> i64 {
    requires: x >= 5;
    ensures: result >= 0;
    body: {
    x
    }
}

atom transfer(x: i64) -> i64 {
    requires: x >= 5;
    ensures: result >= 0;
    body: {
    validate_balance(x)
    }
}

atom withdraw(x: i64) -> i64 {
    requires: x >= 0;
    ensures: result >= 0;
    body: {
    if x >= 0 { 1 } else { 0 }
    }
}
