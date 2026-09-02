type Usd = i64 unit USD;
type Jpy = i64 unit JPY;

struct A { amt: Usd }
struct B { amt: Jpy }

atom bad_branch(c: bool, u: Usd, j: Jpy) -> i64
    requires: true;
    ensures: true;
    body: {
        let p = if c { A { amt: u } } else { B { amt: j } };
        p.amt + u
    };
