// A-1 relational verification (trusted callee): a trusted atom has no
// verified body, so each call inside a spec is a fresh symbol constrained
// only by its declared ensures. `result == oracle(x)` cannot be proven
// from `oracle(x)` in the body: there is no congruence between the two
// fresh symbols, only what `ensures` says about each of them.

trusted atom oracle(x: i64)
    requires: true;
    ensures: result >= x;
    body: x;

atom echo_oracle(x: i64)
    requires: true;
    ensures: result == oracle(x);
    body: oracle(x);
