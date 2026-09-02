// A-1 relational verification (negative): calc_v2 intentionally differs
// from calc_v1 for every x, so `ensures result == calc_v1(x)` must fail
// with a counterexample.

atom calc_v1(x: i64)
    requires: true;
    ensures: result == 2 * x + 1;
    body: 2 * x + 1;

atom calc_v2(x: i64)
    requires: true;
    ensures: result == 2 * x + 2;
    body: 2 * x + 2;

atom v2_matches_v1(x: i64)
    requires: true;
    ensures: result == calc_v1(x);
    body: calc_v2(x);
